//! Connection to Dwarf Fortress through DFHack's RemoteFortressReader.
//!
//! [`GameState`] owns one cached DFHack connection. The version check runs only
//! when a connection is (re)established. Query methods validate their arguments,
//! make synchronous DFHack calls, and shape the data for the AI. The only
//! mutating operation pauses a running fortress when AI processing is blocked.
//! These calls block, so async callers should use [`GameState::run`].
//!
//! The AI should know only what the player knows. Creature information therefore
//! comes from the `rpc.dfcompanion` Lua module (see `lua/rpc/dfcompanion.lua`),
//! which lists the creatures the player is aware of, grouped like the game's
//! Creatures screen. Queries fail rather than fall back to the unfiltered unit
//! list when that module is unavailable.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use dfhack_remote::{BlockRequest, Client, CoreRunLuaRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

use crate::geometry::{
    self, block_bounds, validate_plan_size, Bounds, DEFAULT_PLAN_HEIGHT, DEFAULT_PLAN_WIDTH,
};
use crate::notifications::{DisplayMessage, DisplayMessageKind, Notifier};

const TESTED_DWARF_FORTRESS_VERSION: &str = "53.16";
const TESTED_DFHACK_VERSION: &str = "53.16-r2";
pub const MAX_CREATURE_PAGE_SIZE: usize = 50;
pub const MAX_REPORTS: usize = 50;
pub const MAX_ITEM_PAGE_SIZE: usize = 50;
const MAX_CREATURE_REPLY_BYTES: usize = 12 * 1024;
const LUA_MODULE: &str = "rpc.dfcompanion";
pub const MAX_DETAIL_UNITS: usize = 10;
pub const MAX_CANDIDATES: usize = 25;
const MAX_WORK_DETAIL_OFFSET: usize = 5_000;
const MAX_WORK_ORDER_OFFSET: usize = 5_000;
const MAX_ENCODED_CHAT_BYTES: usize = 2 * 1024 * 1024;
const MAX_GEOMETRY_REPLY_BYTES: usize = 24 * 1024;

fn encode_chat_state(state: &str) -> Result<String, String> {
    if state.len() > crate::ai::MAX_SNAPSHOT_BYTES {
        return Err("Chat history snapshot exceeds the supported size limit.".to_owned());
    }
    let compressed = zstd::stream::encode_all(state.as_bytes(), 0)
        .map_err(|error| format!("Could not compress chat history: {error}"))?;
    let encoded = BASE64.encode(compressed);
    if encoded.len() > MAX_ENCODED_CHAT_BYTES {
        return Err("Encoded chat history exceeds the supported size limit.".to_owned());
    }
    Ok(encoded)
}

fn decode_chat_state(reply: &str) -> Result<Option<String>, String> {
    if reply.is_empty() {
        return Ok(None);
    }
    if let Some(json) = reply.strip_prefix("json:") {
        if json.len() > crate::ai::MAX_SNAPSHOT_BYTES {
            return Err("Saved chat history exceeds the supported size limit.".to_owned());
        }
        return Ok(Some(json.to_owned()));
    }
    let encoded = reply
        .strip_prefix("zstd:")
        .ok_or_else(|| "Saved chat history has an unsupported encoding.".to_owned())?;
    if encoded.len() > MAX_ENCODED_CHAT_BYTES {
        return Err("Encoded chat history exceeds the supported size limit.".to_owned());
    }
    let compressed = BASE64
        .decode(encoded)
        .map_err(|error| format!("Could not decode saved chat history: {error}"))?;
    let decoder = zstd::stream::read::Decoder::new(compressed.as_slice())
        .map_err(|error| format!("Could not decompress saved chat history: {error}"))?;
    let mut bytes = Vec::new();
    decoder
        .take((crate::ai::MAX_SNAPSHOT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not decompress saved chat history: {error}"))?;
    if bytes.len() > crate::ai::MAX_SNAPSHOT_BYTES {
        return Err("Saved chat history exceeds the supported size limit.".to_owned());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| format!("Saved chat history is not UTF-8: {error}"))
}

/// Filters for `find_candidates`; all are optional and combined with AND.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateQuery {
    pub skill_class: Option<String>,
    pub skill: Option<String>,
    pub min_skill_level: Option<i64>,
    pub unassigned_to_squad: Option<bool>,
    pub adults_only: Option<bool>,
    pub healthy: Option<bool>,
    pub min_mood: Option<String>,
    pub profession: Option<String>,
    pub attribute: Option<String>,
    pub min_attribute_value: Option<i64>,
    pub sort_by: Option<String>,
    pub limit: Option<usize>,
}
pub const DETAIL_PARTS: [&str; 7] = [
    "skills",
    "attributes",
    "health",
    "mind",
    "inventory",
    "relations",
    "work",
];
pub const SKILL_CLASSES: [&str; 10] = [
    "normal",
    "medical",
    "personal",
    "social",
    "cultural",
    "military_weapon",
    "military_unarmed",
    "military_attack",
    "military_defense",
    "military_misc",
];

/// The groups of the game's Creatures screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatureGroup {
    Residents,
    PetsAndLivestock,
    Other,
    DeadOrMissing,
}

impl CreatureGroup {
    const ALL: [CreatureGroup; 4] = [
        Self::Residents,
        Self::PetsAndLivestock,
        Self::Other,
        Self::DeadOrMissing,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Residents => "residents",
            Self::PetsAndLivestock => "pets",
            Self::Other => "other",
            Self::DeadOrMissing => "dead",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Residents => "residents",
            Self::PetsAndLivestock => "pets_and_livestock",
            Self::Other => "other_creatures",
            Self::DeadOrMissing => "dead_or_missing",
        }
    }
}

/// The creatures the player knows of, as rows produced by the Lua module.
struct KnownCreatures(BTreeMap<&'static str, Vec<Value>>);

impl KnownCreatures {
    fn parse(json_text: &str) -> Result<Self, String> {
        let mut parsed: Value = serde_json::from_str(json_text).map_err(|error| {
            format!("The DFCompanion Lua module returned invalid data: {error}")
        })?;
        let mut groups = BTreeMap::new();
        for group in CreatureGroup::ALL {
            // The Lua JSON encoder writes an empty list as an empty object.
            let rows = match parsed.get_mut(group.key()).map(Value::take) {
                Some(Value::Array(rows)) => rows,
                Some(Value::Object(map)) if map.is_empty() => Vec::new(),
                _ => {
                    return Err(format!(
                        "The DFCompanion Lua module returned no \"{}\" list.",
                        group.key()
                    ))
                }
            };
            groups.insert(group.key(), rows);
        }
        Ok(Self(groups))
    }

    fn rows(&self, group: CreatureGroup) -> &[Value] {
        &self.0[group.key()]
    }

    fn find(&self, id: i64) -> Option<(CreatureGroup, &Value)> {
        CreatureGroup::ALL.into_iter().find_map(|group| {
            self.rows(group)
                .iter()
                .find(|row| row["id"].as_i64() == Some(id))
                .map(|row| (group, row))
        })
    }
}

fn text_field<'a>(row: &'a Value, field: &str) -> &'a str {
    row[field].as_str().unwrap_or("")
}

/// Unnamed creatures (most animals) have an empty name; omit it instead.
fn tidy_row(row: &Value) -> Value {
    let mut row = row.clone();
    if text_field(&row, "name").is_empty() {
        if let Some(object) = row.as_object_mut() {
            object.remove("name");
        }
    }
    row
}

/// The Lua JSON encoder writes an empty list as an empty object; restore the
/// lists that detail parts can leave empty.
fn restore_empty_lists(details: &mut Value) {
    for path in [
        &["skills"][..],
        &["mind", "recent_thoughts"],
        &["mind", "unmet_needs"],
        &["inventory", "items"],
        &["relations", "affiliations"],
        &["relations", "worshipped_deities"],
        &["work", "work_details"],
    ] {
        let mut node = Some(&mut *details);
        for key in path {
            node = node.and_then(|n| n.get_mut(*key));
        }
        if let Some(node) = node {
            if node.as_object().is_some_and(|map| map.is_empty()) {
                *node = json!([]);
            }
        }
    }
    if let Some(work) = details.get_mut("work") {
        restore_empty_list(&mut work["enabled_labors"]);
        restore_empty_list(&mut work["active_automation"]);
        if let Some(enabled_labors) = work["enabled_labors"].as_array_mut() {
            for labor in enabled_labors {
                restore_empty_list(&mut labor["details"]);
            }
        }
    }
    if let Some(work_details) = details
        .get_mut("work")
        .and_then(|work| work.get_mut("work_details"))
        .and_then(Value::as_array_mut)
    {
        for detail in work_details {
            if let Some(labors) = detail.get_mut("labors") {
                restore_empty_list(labors);
            }
        }
    }
}

fn restore_empty_list(value: &mut Value) {
    if value.as_object().is_some_and(|map| map.is_empty()) {
        *value = json!([]);
    }
}

fn restore_work_detail_lists(value: &mut Value) {
    restore_empty_list(&mut value["work_details"]);
    if let Some(work_details) = value["work_details"].as_array_mut() {
        for detail in work_details {
            if let Some(labors) = detail.get_mut("labors") {
                restore_empty_list(labors);
            }
            if let Some(assigned_units) = detail.get_mut("assigned_units") {
                restore_empty_list(assigned_units);
            }
        }
    }
}

fn restore_work_order_lists(value: &mut Value) {
    restore_empty_list(&mut value["work_orders"]);
    if let Some(work_orders) = value["work_orders"].as_array_mut() {
        for order in work_orders {
            restore_empty_list(&mut order["item_conditions"]);
            restore_empty_list(&mut order["order_conditions"]);
            if let Some(conditions) = order["item_conditions"].as_array_mut() {
                for condition in conditions {
                    if condition.get("contains").is_some() {
                        restore_empty_list(&mut condition["contains"]);
                    }
                }
            }
        }
    }
}

fn restore_reaction_lists(value: &mut Value) {
    restore_empty_list(&mut value["reactions"]);
    if let Some(reactions) = value["reactions"].as_array_mut() {
        for reaction in reactions {
            for key in ["allowed_buildings", "reagents", "products", "flags"] {
                restore_empty_list(&mut reaction[key]);
            }
            if let Some(reagents) = reaction["reagents"].as_array_mut() {
                for reagent in reagents {
                    for key in ["flags", "reagent_flags"] {
                        if reagent.get(key).is_some() {
                            restore_empty_list(&mut reagent[key]);
                        }
                    }
                    if reagent.get("contains").is_some() {
                        restore_empty_list(&mut reagent["contains"]);
                    }
                }
            }
        }
    }
}

fn restore_workshop_lists(value: &mut Value) {
    restore_empty_list(&mut value["workshops"]);
    if let Some(workshops) = value["workshops"].as_array_mut() {
        for workshop in workshops {
            restore_stockpile_links(workshop);
            restore_empty_list(&mut workshop["jobs"]);
            if let Some(jobs) = workshop["jobs"].as_array_mut() {
                for job in jobs {
                    restore_empty_list(&mut job["wait_reasons"]);
                    restore_empty_list(&mut job["flags"]);
                    restore_empty_list(&mut job["requirements"]);
                    if let Some(requirements) = job["requirements"].as_array_mut() {
                        for requirement in requirements {
                            restore_empty_list(&mut requirement["flags"]);
                        }
                    }
                }
            }
        }
    }
}

fn restore_stockpile_links(row: &mut Value) {
    if let Some(links) = row.get_mut("stockpile_links") {
        for key in [
            "give_to_stockpiles",
            "take_from_stockpiles",
            "give_to_workshops",
            "take_from_workshops",
        ] {
            restore_empty_list(&mut links[key]);
        }
    }
}

fn restore_stockpile_lists(value: &mut Value) {
    restore_empty_list(&mut value["stockpiles"]);
    if let Some(stockpiles) = value["stockpiles"].as_array_mut() {
        for stockpile in stockpiles {
            restore_stockpile_links(stockpile);
        }
    }
}

fn contains_text(value: &Value, search: &str) -> bool {
    match value {
        Value::String(text) => text.to_lowercase().contains(search),
        Value::Array(values) => values.iter().any(|value| contains_text(value, search)),
        Value::Object(values) => values.values().any(|value| contains_text(value, search)),
        _ => false,
    }
}

fn collect_production_reaction_codes(value: &Value, codes: &mut BTreeSet<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_production_reaction_codes(value, codes);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if key == "reaction" {
                    if let Some(code) = value.as_str() {
                        codes.insert(code.to_owned());
                    }
                } else {
                    collect_production_reaction_codes(value, codes);
                }
            }
        }
        _ => {}
    }
}

fn filter_production_evidence(
    workshops: &[Value],
    work_orders: &[Value],
    search: Option<&str>,
    workshop_id: Option<i64>,
    order_id: Option<i64>,
) -> (Vec<Value>, Vec<Value>) {
    let search = search
        .map(str::trim)
        .filter(|search| !search.is_empty())
        .map(str::to_lowercase);
    let mut matching_jobs = Vec::new();
    for workshop in workshops {
        if workshop_id.is_some_and(|id| workshop["id"].as_i64() != Some(id)) {
            continue;
        }
        let workshop_matches = search.as_deref().is_some_and(|search| {
            workshop["name"]
                .as_str()
                .is_some_and(|name| name.to_lowercase().contains(search))
        });
        if let Some(jobs) = workshop["jobs"].as_array() {
            for job in jobs {
                if order_id.is_some_and(|id| job["order_id"].as_i64() != Some(id))
                    || search
                        .as_deref()
                        .is_some_and(|search| !workshop_matches && !contains_text(job, search))
                {
                    continue;
                }
                matching_jobs.push(job.clone());
            }
        }
    }
    let linked_order_ids = matching_jobs
        .iter()
        .filter_map(|job| job["order_id"].as_i64())
        .collect::<BTreeSet<_>>();

    let selected_orders = work_orders
        .iter()
        .filter(|order| {
            let id = order["id"].as_i64();
            let linked_to_job = id.is_some_and(|id| linked_order_ids.contains(&id));
            workshop_id.is_none_or(|workshop_id| {
                order["workshop_id"].as_i64() == Some(workshop_id) || linked_to_job
            }) && order_id.is_none_or(|order_id| id == Some(order_id))
                && search
                    .as_deref()
                    .is_none_or(|search| contains_text(order, search) || linked_to_job)
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected_order_ids = selected_orders
        .iter()
        .filter_map(|order| order["id"].as_i64())
        .collect::<BTreeSet<_>>();

    let mut selected_workshops = Vec::new();
    for workshop in workshops {
        if workshop_id.is_some_and(|id| workshop["id"].as_i64() != Some(id)) {
            continue;
        }
        let workshop_matches = search.as_deref().is_some_and(|search| {
            workshop["name"]
                .as_str()
                .is_some_and(|name| name.to_lowercase().contains(search))
        });
        let mut selected = workshop.clone();
        if let Some(jobs) = selected["jobs"].as_array_mut() {
            jobs.retain(|job| {
                let id = job["order_id"].as_i64();
                order_id.is_none_or(|order_id| id == Some(order_id))
                    && (search.is_none()
                        || workshop_matches
                        || contains_text(job, search.as_deref().unwrap_or_default())
                        || id.is_some_and(|id| selected_order_ids.contains(&id)))
            });
        }
        let has_selected_jobs = selected["jobs"]
            .as_array()
            .is_some_and(|jobs| !jobs.is_empty());
        if has_selected_jobs
            || workshop_id.is_some()
            || workshop_matches
            || selected["jobs"].as_array().is_none()
        {
            selected_workshops.push(selected);
        }
    }

    (selected_workshops, selected_orders)
}

fn production_block_signals(workshops: &[Value], work_orders: &[Value]) -> Vec<Value> {
    let mut signals = Vec::new();
    for workshop in workshops {
        if let Some(jobs) = workshop["jobs"].as_array() {
            for job in jobs {
                if let Some(reasons) = job["wait_reasons"].as_array() {
                    for reason in reasons.iter().filter_map(Value::as_str) {
                        signals.push(json!({
                            "source": "queued_job",
                            "signal": "wait_reason",
                            "value": reason,
                            "workshop_id": workshop["id"],
                            "job_id": job["id"],
                        }));
                    }
                }
                if job["remaining_dimension"]
                    .as_i64()
                    .is_some_and(|remaining| remaining > 0)
                {
                    signals.push(json!({
                        "source": "queued_job",
                        "signal": "items_not_yet_delivered",
                        "remaining_dimension": job["remaining_dimension"],
                        "workshop_id": workshop["id"],
                        "job_id": job["id"],
                    }));
                }
            }
        }
    }
    for order in work_orders {
        if order["is_active"] == false {
            signals.push(json!({
                "source": "manager_order",
                "signal": "inactive",
                "order_id": order["id"],
            }));
        }
        if order["is_validated"] == false {
            signals.push(json!({
                "source": "manager_order",
                "signal": "not_validated",
                "order_id": order["id"],
            }));
        }
        if let Some(conditions) = order["order_conditions"].as_array() {
            for condition in conditions
                .iter()
                .filter(|condition| condition["satisfied"] == false)
            {
                signals.push(json!({
                    "source": "manager_order",
                    "signal": "unsatisfied_order_condition",
                    "order_id": order["id"],
                    "condition": condition["condition"],
                }));
            }
        }
    }
    signals
}

fn validate_detail_parts(include: &[String]) -> Result<(), String> {
    if let Some(part) = include.iter().find(|p| !DETAIL_PARTS.contains(&p.as_str())) {
        return Err(format!(
            "Unknown part \"{part}\"; use any of: {}.",
            DETAIL_PARTS.join(", ")
        ));
    }
    Ok(())
}

fn count_by_race(rows: &[Value]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts
            .entry(text_field(row, "race").to_owned())
            .or_default() += 1;
    }
    counts
}

fn matches_search(row: &Value, search: &str) -> bool {
    ["name", "race", "profession"]
        .iter()
        .any(|field| text_field(row, field).to_lowercase().contains(search))
}

#[derive(Default)]
struct Connection {
    client: Option<Client>,
    last_version_warning: Option<String>,
}

/// How far the game has progressed through loading, ordered from least to most
/// usable. Tracking it lets tools that read fort data stay inert until a
/// fortress is loaded, without pausing the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LifecycleState {
    /// DFHack could not be reached, so nothing is known. Also the initial
    /// state before the first check.
    #[default]
    Unreachable,
    /// DFHack is reachable but no world is loaded (for example, the title
    /// screen or world generation).
    Initialized,
    /// A world is loaded but no map (for example, legends mode).
    WorldLoaded,
    /// A fortress map is loaded. The only state in which fort tools are safe.
    MapLoaded,
}

/// The game's loading state plus the last fortress name seen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LifecycleStatus {
    pub state: LifecycleState,
    /// The last non-empty fortress name observed, kept across an unload so the
    /// UI can still name the fortress the player was in.
    pub fortress_name: Option<String>,
    /// Stable identity of the last loaded fortress site.
    pub site_id: Option<i64>,
    /// Identity of the last observed fortress activation.
    pub session_id: Option<String>,
}

/// A single reading of the game's loading state, as returned by the Lua module.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleSnapshot {
    pub world_loaded: bool,
    pub map_loaded: bool,
    pub fortress_name: String,
    pub site_id: Option<i64>,
    pub session_id: Option<String>,
}

impl LifecycleSnapshot {
    fn validate(&self) -> Result<(), String> {
        let valid_session = self.session_id.as_ref().is_some_and(|session| {
            !session.is_empty()
                && session.len() <= 64
                && session
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        });
        if self.map_loaded {
            if !self.world_loaded || self.site_id.is_none() || !valid_session {
                return Err("Loaded fortress lifecycle is missing a valid site or session.".into());
            }
        } else if self.site_id.is_some() || self.session_id.is_some() {
            return Err("Unloaded fortress lifecycle contains a site or session.".into());
        }
        Ok(())
    }
}

/// Cheaply cloneable handle to the shared DFHack connection.
#[derive(Clone)]
pub struct GameState {
    connection: Arc<Mutex<Connection>>,
    notifier: Notifier,
    lifecycle: watch::Receiver<LifecycleStatus>,
}

impl GameState {
    pub fn new(notifier: Notifier, lifecycle: watch::Receiver<LifecycleStatus>) -> Self {
        Self {
            connection: Arc::default(),
            notifier,
            lifecycle,
        }
    }

    /// Whether a fortress map is loaded, so tools that read fort data are safe
    /// to run. Reads a cached value; it makes no DFHack call of its own.
    pub fn map_loaded(&self) -> bool {
        self.lifecycle.borrow().state >= LifecycleState::MapLoaded
    }

    /// Asks DFHack how far it has loaded. Safe to call at any screen, including
    /// before a world exists: it reads only the loaded/not-loaded flags and the
    /// current site's name.
    pub fn query_lifecycle(&self) -> Result<LifecycleSnapshot, String> {
        let reply = self
            .with_client("Could not query the game's loading state", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "lifecycle_state".to_owned(),
                    arguments: Vec::new(),
                })
            })?
            .reply
            .value;
        let snapshot: LifecycleSnapshot = serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the game state reply: {error}"))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Loads the raw DFCompanion chat state associated with `site_id`.
    #[tracing::instrument(level = "trace", skip(self), fields(site_id), err)]
    pub fn load_chat_state(&self, site_id: i64) -> Result<Option<String>, String> {
        let reply = self
            .with_client("Could not load saved chat history", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "load_chat_state".to_owned(),
                    arguments: vec![site_id.to_string()],
                })
            })?
            .reply
            .value
            .concat();
        decode_chat_state(&reply)
    }

    /// Replaces the raw DFCompanion chat state associated with `site_id`.
    ///
    /// The Lua side checks the site again immediately before writing so a
    /// delayed request cannot save one fortress's state into another.
    #[tracing::instrument(
        level = "trace",
        skip(self, state),
        fields(site_id, state_bytes = state.len()),
        err
    )]
    pub fn save_chat_state(&self, site_id: i64, state: &str) -> Result<(), String> {
        let state = format!("zstd:{}", encode_chat_state(state)?);
        let result = self
            .with_client("Could not save chat history", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "save_chat_state".to_owned(),
                    arguments: vec![site_id.to_string(), state.clone()],
                })
            })?
            .reply
            .value
            .concat();
        parse_lua_boolean_result(&result, "save_chat_state").and_then(|saved| {
            saved
                .then_some(())
                .ok_or_else(|| "DFHack reported that chat history was not saved.".to_owned())
        })
    }

    /// Runs a blocking query on the blocking thread pool.
    pub async fn run<T, F>(&self, query: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&GameState) -> Result<T, String> + Send + 'static,
    {
        let game_state = self.clone();
        tokio::task::spawn_blocking(move || query(&game_state))
            .await
            .map_err(|error| format!("DFHack query task failed: {error}"))?
    }

    /// Runs `call` with the cached client, connecting (and checking versions)
    /// first when needed. A call that fails because the link is broken discards
    /// the connection. If that connection was an older one, the call is retried
    /// once on a fresh connection, since the game may have been restarted.
    fn with_client<T>(
        &self,
        what: &str,
        mut call: impl FnMut(&mut Client) -> dfhack_remote::Result<T>,
    ) -> Result<T, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|error| format!("DFHack connection state is unavailable: {error}"))?;
        let mut may_retry = connection.client.is_some();
        loop {
            if connection.client.is_none() {
                self.establish(&mut connection)?;
            }
            let client = connection
                .client
                .as_mut()
                .expect("connection was just established");
            match call(client) {
                Ok(value) => return Ok(value),
                Err(error) if is_link_failure(&error) => {
                    connection.client = None;
                    if !may_retry {
                        return Err(format!("{what}: {error}"));
                    }
                    may_retry = false;
                }
                Err(error) => return Err(format!("{what}: {error}")),
            }
        }
    }

    fn establish(&self, connection: &mut Connection) -> Result<(), String> {
        let mut client = dfhack_remote::connect().map_err(|error| {
            format!("Could not connect to the DFHack remote interface: {error}")
        })?;
        let version = client
            .remote_fortress_reader()
            .get_version_info()
            .map_err(|error| format!("Could not check DFHack/game version: {error}"))?
            .reply;
        let warning = version_mismatch_warning(
            version.dwarf_fortress_version.as_deref(),
            version.dfhack_version.as_deref(),
        );
        notify_version_warning(
            warning,
            &mut connection.last_version_warning,
            &self.notifier,
        );
        connection.client = Some(client);
        Ok(())
    }

    fn known_creatures(&self, details: Option<&Value>) -> Result<KnownCreatures, String> {
        let reply = self
            .with_client(
                "Could not determine which creatures the player knows of (is the DFCompanion Lua module installed?)",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "known_creatures".to_owned(),
                        arguments: details.map(|options| vec![options.to_string()]).unwrap_or_default(),
                    })
                },
            )?
            .reply
            .value;
        KnownCreatures::parse(&reply.concat())
    }

    pub(crate) fn game_date(&self) -> Result<String, String> {
        let reply = self
            .with_client("Could not retrieve the game date", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "game_date".to_owned(),
                    arguments: Vec::new(),
                })
            })?
            .reply
            .value;
        Ok(reply.concat())
    }

    fn z_offset(&self) -> Result<i64, String> {
        let reply = self
            .with_client("Could not retrieve the display z offset", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "z_offset".to_owned(),
                    arguments: Vec::new(),
                })
            })?
            .reply
            .value;
        reply
            .concat()
            .trim()
            .parse()
            .map_err(|error| format!("Could not parse the display z offset: {error}"))
    }

    /// A one-line description of the connected game, used to verify the link
    /// at startup.
    pub fn startup_summary(&self) -> Result<String, String> {
        let paused = self.is_paused()?;
        let date = self.game_date()?;
        let name = self
            .with_client("Could not retrieve the fortress name", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "fortress_name".to_owned(),
                    arguments: Vec::new(),
                })
            })?
            .reply
            .value
            .concat();
        let name = if name.is_empty() {
            "(no fortress loaded)".to_owned()
        } else {
            name
        };
        let script_enabled = self
            .with_client(
                "Could not check whether the DFCompanion script is enabled",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "script_enabled".to_owned(),
                        arguments: Vec::new(),
                    })
                },
            )?
            .reply
            .value
            .concat()
            == "true";
        let events = if script_enabled {
            "The DFCompanion script is enabled and forwarding game events."
        } else {
            "The DFCompanion script is not enabled, so game events will not be forwarded; enter `enable dfcompanion` in the DFHack console. Tools still work."
        };
        Ok(format!(
            "Connected to Dwarf Fortress. Fortress: {name}. Date: {date}. Game is {}. {events}",
            if paused { "paused" } else { "running" }
        ))
    }

    /// Whether the game is paused (a single cheap call, for scheduling).
    pub fn is_paused(&self) -> Result<bool, String> {
        let reply = self
            .with_client("Could not retrieve pause state", |client| {
                client.remote_fortress_reader().get_pause_state()
            })?
            .reply;
        Ok(reply)
    }

    /// Pauses a loaded, running fortress and records why so the Lua plugin can
    /// notify the player. Returns false if the fortress was already paused.
    pub fn pause_for_ai_block(&self, reason: &str) -> Result<bool, String> {
        let result = self
            .with_client(
                "Could not pause Dwarf Fortress for blocked AI processing",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "pause_for_ai_block".to_owned(),
                        arguments: vec![reason.to_owned()],
                    })
                },
            )?
            .reply
            .value
            .concat();
        parse_lua_boolean_result(&result, "pause_for_ai_block")
    }

    /// Returns a high-level snapshot: world, save, pause state, view, and the
    /// number of known creatures in each group.
    pub fn fortress_overview(&self) -> Result<Value, String> {
        let map = self
            .with_client("Could not retrieve map info", |client| {
                client.remote_fortress_reader().get_map_info()
            })?
            .reply;
        let paused = self
            .with_client("Could not retrieve pause state", |client| {
                client.remote_fortress_reader().get_pause_state()
            })?
            .reply;
        let view = self
            .with_client("Could not retrieve view info", |client| {
                client.remote_fortress_reader().get_view_info()
            })?
            .reply;
        let creatures = self.known_creatures(None)?;
        let game_date = self.game_date()?;
        let z_offset = self.z_offset()?;
        let counts = CreatureGroup::ALL
            .into_iter()
            .map(|group| (group.label(), creatures.rows(group).len()))
            .collect::<BTreeMap<_, _>>();
        Ok(json!({
            "world_name": map.world_name_english.or(map.world_name),
            "save_name": map.save_name,
            "paused": paused,
            "game_date": game_date,
            "map_size_in_blocks": map.block_size_x.zip(map.block_size_y).zip(map.block_size_z)
                .map(|((x, y), z)| json!({"x": x, "y": y, "z": z})),
            "view_position": view.view_pos_x.zip(view.view_pos_y).zip(view.view_pos_z)
                .map(|((x, y), z)| json!({"x": x, "y": y, "z": i64::from(z) + z_offset})),
            "followed_unit_id": view.follow_unit_id.filter(|id| *id >= 0),
            "known_creature_counts": counts,
            "pets_and_livestock_by_race": count_by_race(creatures.rows(CreatureGroup::PetsAndLivestock)),
        }))
    }

    /// Summarizes where the fortress's known places are distributed by UI z
    /// level, without returning a raw tile map.
    pub fn geometry_overview(&self) -> Result<Value, String> {
        let mut result = self.call_lua_json(
            "summarize fortress geometry",
            "geometry_overview",
            Vec::new(),
        )?;
        let map = self
            .with_client("Could not retrieve map dimensions for geometry", |client| {
                client.remote_fortress_reader().get_map_info()
            })?
            .reply;
        let view = self
            .with_client(
                "Could not retrieve the current view for geometry",
                |client| client.remote_fortress_reader().get_view_info(),
            )?
            .reply;
        let z_offset = self.z_offset()?;
        result["map_size_in_tiles"] = map
            .block_size_x
            .zip(map.block_size_y)
            .zip(map.block_size_z)
            .map(|((x, y), z)| json!({"x": x * 16, "y": y * 16, "z": z}))
            .unwrap_or(Value::Null);
        result["view_position"] = view
            .view_pos_x
            .zip(view.view_pos_y)
            .zip(view.view_pos_z)
            .map(|((x, y), z)| json!({"x": x, "y": y, "z": i64::from(z) + z_offset}))
            .unwrap_or(Value::Null);
        result["view_size"] = view
            .view_size_x
            .zip(view.view_size_y)
            .map(|(x, y)| json!({"x": x, "y": y}))
            .unwrap_or(Value::Null);
        if result["levels"]
            .as_object()
            .is_some_and(|levels| levels.is_empty())
        {
            result["levels"] = json!([]);
        }
        Ok(result)
    }

    /// Returns a bounded player-visible ASCII floor plan and nearby places.
    /// Coordinates and z levels in the result use the game's UI coordinate
    /// convention; omitted x/y centers the plan on the current game view.
    pub fn floor_plan(
        &self,
        z: i64,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<i32>,
        height: Option<i32>,
    ) -> Result<Value, String> {
        let width = width.unwrap_or(DEFAULT_PLAN_WIDTH);
        let height = height.unwrap_or(DEFAULT_PLAN_HEIGHT);
        validate_plan_size(width, height)?;

        let map = self
            .with_client("Could not retrieve map dimensions for geometry", |client| {
                client.remote_fortress_reader().get_map_info()
            })?
            .reply;
        let map_width = map
            .block_size_x
            .ok_or_else(|| "DFHack did not report the map width.".to_owned())?
            .checked_mul(16)
            .ok_or_else(|| "The map width is too large.".to_owned())?;
        let map_height = map
            .block_size_y
            .ok_or_else(|| "DFHack did not report the map height.".to_owned())?
            .checked_mul(16)
            .ok_or_else(|| "The map height is too large.".to_owned())?;
        let map_z = map
            .block_size_z
            .ok_or_else(|| "DFHack did not report the map depth.".to_owned())?;
        let view = self
            .with_client(
                "Could not retrieve the current view for geometry",
                |client| client.remote_fortress_reader().get_view_info(),
            )?
            .reply;
        let default_center_x = view
            .view_pos_x
            .zip(view.view_size_x)
            .map(|(position, size)| position + size / 2);
        let default_center_y = view
            .view_pos_y
            .zip(view.view_size_y)
            .map(|(position, size)| position + size / 2);
        let center_x = x.or(default_center_x).ok_or_else(|| {
            "DFHack did not report an x coordinate for the current view.".to_owned()
        })?;
        let center_y = y.or(default_center_y).ok_or_else(|| {
            "DFHack did not report a y coordinate for the current view.".to_owned()
        })?;
        if x.is_some_and(|x| !(0..map_width).contains(&x)) {
            return Err(format!(
                "x must be between 0 and {} for this map.",
                map_width - 1
            ));
        }
        if y.is_some_and(|y| !(0..map_height).contains(&y)) {
            return Err(format!(
                "y must be between 0 and {} for this map.",
                map_height - 1
            ));
        }

        let z_offset = self.z_offset()?;
        let internal_z = z
            .checked_sub(z_offset)
            .ok_or_else(|| "The z coordinate is outside the supported range.".to_owned())?;
        let internal_z = i32::try_from(internal_z)
            .map_err(|_| "The z coordinate is outside the supported range.".to_owned())?;
        if !(0..map_z).contains(&internal_z) {
            return Err(format!(
                "z must be between {z_offset} and {} for this map.",
                i64::from(map_z - 1) + z_offset
            ));
        }

        let requested = Bounds::from_center(center_x, center_y, width, height);
        let bounds = requested.clamp(map_width, map_height);
        let (min_block_x, min_block_y, max_block_x, max_block_y) = block_bounds(bounds);
        let blocks_needed = (max_block_x - min_block_x) * (max_block_y - min_block_y);
        let blocks = self
            .with_client("Could not retrieve fortress map blocks", |client| {
                client
                    .remote_fortress_reader()
                    .get_block_list(BlockRequest {
                        blocks_needed: Some(blocks_needed),
                        min_x: Some(min_block_x),
                        max_x: Some(max_block_x),
                        min_y: Some(min_block_y),
                        max_y: Some(max_block_y),
                        min_z: Some(internal_z),
                        max_z: Some(internal_z + 1),
                        force_reload: Some(true),
                    })
            })?
            .reply;
        let tiletypes = self
            .with_client("Could not retrieve fortress tile types", |client| {
                client.remote_fortress_reader().get_tiletype_list()
            })?
            .reply
            .tiletype_list;
        let mut result = geometry::floor_plan(&blocks, &tiletypes, bounds, z)?;

        let mut places = self.call_lua_json(
            "list places in the geometry area",
            "geometry_places",
            vec![
                bounds.x1.to_string(),
                bounds.y1.to_string(),
                bounds.x2.to_string(),
                bounds.y2.to_string(),
                z.to_string(),
            ],
        )?;
        if places["places"]
            .as_object()
            .is_some_and(|map| map.is_empty())
        {
            places["places"] = json!([]);
        }
        result["places"] = places["places"].take();
        result["more_places"] = places["more_places"].take();
        result["centered_at"] = json!({"x": center_x, "y": center_y});
        result["clipped_to_map"] = json!(bounds != requested);
        let encoded_bytes = result.to_string().len();
        if encoded_bytes > MAX_GEOMETRY_REPLY_BYTES {
            return Err(format!(
                "The floor plan reply exceeded the {MAX_GEOMETRY_REPLY_BYTES}-byte limit. Request a smaller area."
            ));
        }
        Ok(result)
    }

    /// Describes the player-visible terrain and objects at one UI-coordinate
    /// tile. Hidden tiles return only the requested position and a redaction note.
    pub fn inspect_tile(&self, x: i32, y: i32, z: i64) -> Result<Value, String> {
        if x < 0 || y < 0 {
            return Err("x and y must be non-negative map coordinates.".to_owned());
        }
        self.call_lua_json(
            "inspect tile",
            "inspect_tile",
            vec![x.to_string(), y.to_string(), z.to_string()],
        )
    }

    /// Returns information about up to `MAX_DETAIL_UNITS` creatures the player
    /// knows of: everything the lists show plus the requested `include` parts.
    /// Creatures the player does not know of are listed in `not_found`.
    pub fn unit_details(
        &self,
        unit_ids: &[i32],
        include: &[String],
        skill_class: Option<&str>,
        min_skill_level: Option<i64>,
    ) -> Result<Value, String> {
        if !(1..=MAX_DETAIL_UNITS).contains(&unit_ids.len()) {
            return Err(format!("Give between 1 and {MAX_DETAIL_UNITS} unit IDs."));
        }
        validate_detail_parts(include)?;
        if let Some(class) = skill_class {
            if !SKILL_CLASSES.contains(&class) {
                return Err(format!(
                    "Unknown skill class \"{class}\"; use one of: {}.",
                    SKILL_CLASSES.join(", ")
                ));
            }
        }
        let parts = include
            .iter()
            .map(|part| (part.as_str(), true))
            .collect::<BTreeMap<_, _>>();
        let options = json!({
            "ids": unit_ids,
            "parts": parts,
            "skill_class": skill_class,
            "min_skill_level": min_skill_level,
        });
        let creatures = self.known_creatures(Some(&options))?;
        let mut units = Vec::new();
        let mut not_found = Vec::new();
        for &id in unit_ids {
            match creatures.find(i64::from(id)) {
                Some((group, row)) => {
                    let mut details = tidy_row(row);
                    details["group"] = json!(group.label());
                    restore_empty_lists(&mut details);
                    units.push(details);
                }
                None => not_found.push(id),
            }
        }
        if units.is_empty() {
            return Err(format!("No known creature with ID {}.", not_found[0]));
        }
        let mut result = json!({"units": units});
        if !not_found.is_empty() {
            result["not_found"] = json!(not_found);
        }
        Ok(result)
    }

    /// Returns the newest `limit` announcements, oldest first, optionally
    /// restricted to the inclusive game-date range `from`..`to`
    /// ("YYYY-MM-DD" or "YYYY-MM-DD:T") and to report ids in
    /// `since_id < id <= until_id`. With `since_id`, returns the oldest
    /// `limit` matches instead, so callers can page forward without gaps.
    pub fn recent_reports(
        &self,
        limit: usize,
        from: Option<&str>,
        to: Option<&str>,
        category: Option<&str>,
        since_id: Option<i64>,
        until_id: Option<i64>,
    ) -> Result<Value, String> {
        if !(1..=MAX_REPORTS).contains(&limit) {
            return Err(format!(
                "The report limit must be between 1 and {MAX_REPORTS}."
            ));
        }
        for (name, value) in [("from", from), ("to", to)] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                validate_game_date(value).map_err(|error| format!("Invalid {name}: {error}"))?;
            }
        }
        let reply = self
            .with_client(
                "Could not retrieve game reports (is the DFCompanion Lua module installed?)",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "recent_reports".to_owned(),
                        arguments: vec![
                            limit.to_string(),
                            from.unwrap_or_default().to_owned(),
                            to.unwrap_or_default().to_owned(),
                            category.unwrap_or_default().to_owned(),
                            since_id.map(|id| id.to_string()).unwrap_or_default(),
                            until_id.map(|id| id.to_string()).unwrap_or_default(),
                        ],
                    })
                },
            )?
            .reply
            .value;
        serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the game reports: {error}"))
    }

    /// Returns food and drink stocks with rough days of supply (see
    /// `food_drink_summary` in the Lua module).
    pub fn food_drink_summary(&self) -> Result<Value, String> {
        let reply = self
            .with_client(
                "Could not summarize food and drink (is the DFCompanion Lua module installed?)",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "food_drink_summary".to_owned(),
                        arguments: Vec::new(),
                    })
                },
            )?
            .reply
            .value;
        serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the food and drink summary: {error}"))
    }

    /// Calls a Lua function in the DFCompanion module with string arguments and
    /// parses its JSON reply, turning an `error` field into `Err`.
    fn call_lua_json(
        &self,
        what: &str,
        function: &str,
        arguments: Vec<String>,
    ) -> Result<Value, String> {
        let reply = self
            .with_client(
                &format!("Could not {what} (is the DFCompanion Lua module installed?)"),
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: function.to_owned(),
                        arguments: arguments.clone(),
                    })
                },
            )?
            .reply
            .value;
        let value: Value = serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the {what} reply: {error}"))?;
        match value.get("error").and_then(Value::as_str) {
            Some(message) => Err(message.to_owned()),
            None => Ok(value),
        }
    }

    /// Residents matching all the filters in `query`, best first (see
    /// `find_candidates` in the Lua module).
    pub fn find_candidates(&self, query: &CandidateQuery) -> Result<Value, String> {
        if let Some(class) = query.skill_class.as_deref() {
            if !SKILL_CLASSES.contains(&class) {
                return Err(format!(
                    "Unknown skill class \"{class}\"; use one of: {}.",
                    SKILL_CLASSES.join(", ")
                ));
            }
        }
        if query
            .limit
            .is_some_and(|limit| !(1..=MAX_CANDIDATES).contains(&limit))
        {
            return Err(format!("limit must be between 1 and {MAX_CANDIDATES}."));
        }
        let mut options = serde_json::to_value(query).map_err(|error| error.to_string())?;
        if let Some(map) = options.as_object_mut() {
            map.retain(|_, value| !value.is_null());
        }
        let mut result = self.call_lua_json(
            "search for candidates",
            "find_candidates",
            vec![options.to_string()],
        )?;
        if result["candidates"]
            .as_object()
            .is_some_and(|map| map.is_empty())
        {
            result["candidates"] = json!([]);
        }
        Ok(result)
    }

    /// Housing summary with one paged list (see `housing` in the Lua module).
    pub fn housing(
        &self,
        list: &str,
        z: Option<i64>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        self.call_lua_json(
            "describe housing",
            "housing",
            vec![
                list.to_owned(),
                z.map(|z| z.to_string()).unwrap_or_default(),
                offset.to_string(),
                limit.to_string(),
            ],
        )
    }

    /// A page of non-bedroom zones (see `list_zones` in the Lua module).
    pub fn list_zones(
        &self,
        zone_type: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        self.call_lua_json(
            "list zones",
            "list_zones",
            vec![
                zone_type.unwrap_or_default().to_owned(),
                offset.to_string(),
                limit.to_string(),
            ],
        )
    }

    /// Squads with members, defined uniforms and what each member carries.
    pub fn list_squads(
        &self,
        name: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        self.call_lua_json(
            "list squads",
            "list_squads",
            vec![
                name.unwrap_or_default().to_owned(),
                offset.to_string(),
                limit.to_string(),
            ],
        )
    }

    /// Stockpiles with position, accepted categories, fullness and contents.
    pub fn list_stockpiles(
        &self,
        category: Option<&str>,
        name: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        let mut result = self.call_lua_json(
            "list stockpiles",
            "list_stockpiles",
            vec![
                category.unwrap_or_default().to_owned(),
                name.unwrap_or_default().to_owned(),
                offset.to_string(),
                limit.to_string(),
            ],
        )?;
        restore_stockpile_lists(&mut result);
        Ok(result)
    }

    /// Named locations such as temples and hospitals.
    pub fn list_locations(&self) -> Result<Value, String> {
        self.call_lua_json("list locations", "list_locations", Vec::new())
    }

    /// Work-detail assignment configuration, optionally filtered by name/labor
    /// or restricted to details relevant to a known resident.
    pub fn list_work_details(
        &self,
        search: Option<&str>,
        labor: Option<&str>,
        unit_id: Option<i32>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        if offset > MAX_WORK_DETAIL_OFFSET {
            return Err("offset exceeds the maximum supported list index.".to_owned());
        }
        if unit_id.is_some_and(|id| id < 0) {
            return Err("unit_id must be a non-negative known resident ID.".to_owned());
        }
        let mut result = self.call_lua_json(
            "list work details",
            "list_work_details",
            vec![
                search.unwrap_or_default().to_owned(),
                unit_id.map(|id| id.to_string()).unwrap_or_default(),
                labor.unwrap_or_default().to_owned(),
                offset.to_string(),
                limit.to_string(),
            ],
        )?;
        restore_work_detail_lists(&mut result);
        Ok(result)
    }

    /// Known residents with a current native enablement for one labor, plus
    /// the work details that apply to each resident.
    pub fn labor_coverage(
        &self,
        labor: &str,
        idle_only: bool,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        if offset > MAX_WORK_DETAIL_OFFSET {
            return Err("offset exceeds the maximum supported list index.".to_owned());
        }
        let mut result = self.call_lua_json(
            "check labor coverage",
            "labor_coverage",
            vec![
                labor.to_owned(),
                idle_only.to_string(),
                offset.to_string(),
                limit.to_string(),
            ],
        )?;
        restore_empty_list(&mut result["covering_details"]);
        restore_empty_list(&mut result["residents"]);
        if let Some(residents) = result["residents"].as_array_mut() {
            for resident in residents {
                restore_empty_list(&mut resident["applicable_details"]);
            }
        }
        Ok(result)
    }

    /// A page of fortress manager/work orders and their stored conditions.
    pub fn list_work_orders(
        &self,
        search: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        if offset > MAX_WORK_ORDER_OFFSET {
            return Err("offset exceeds the maximum supported list index.".to_owned());
        }
        let mut result = self.call_lua_json(
            "list work orders",
            "list_work_orders",
            vec![
                search.unwrap_or_default().to_owned(),
                offset.to_string(),
                limit.to_string(),
            ],
        )?;
        restore_work_order_lists(&mut result);
        Ok(result)
    }

    /// Looks up a reaction definition by exact code or by text search.
    pub fn get_reaction(
        &self,
        code: Option<&str>,
        search: Option<&str>,
        available_only: bool,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        if offset > MAX_WORK_ORDER_OFFSET {
            return Err("offset exceeds the maximum supported list index.".to_owned());
        }
        let mut result = self.call_lua_json(
            "get reaction",
            "get_reaction",
            vec![
                code.unwrap_or_default().to_owned(),
                search.unwrap_or_default().to_owned(),
                available_only.to_string(),
                offset.to_string(),
                limit.to_string(),
            ],
        )?;
        restore_reaction_lists(&mut result);
        Ok(result)
    }

    /// A page of workshops and furnaces (see `list_workshops` in the Lua module).
    pub fn list_workshops(
        &self,
        name: Option<&str>,
        only_with_jobs: bool,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        check_page(limit)?;
        let mut result = self.call_lua_json(
            "list workshops",
            "list_workshops",
            vec![
                name.unwrap_or_default().to_owned(),
                only_with_jobs.to_string(),
                offset.to_string(),
                limit.to_string(),
            ],
        )?;
        restore_workshop_lists(&mut result);
        Ok(result)
    }

    /// Combines the read-only evidence needed to diagnose a blocked production
    /// order without inferring a root cause that DF does not expose.
    pub fn diagnose_production_block(
        &self,
        search: Option<&str>,
        workshop_id: Option<i64>,
        order_id: Option<i64>,
    ) -> Result<Value, String> {
        if search.is_none_or(|search| search.trim().is_empty())
            && workshop_id.is_none()
            && order_id.is_none()
        {
            return Err("Provide search, workshop_id, or order_id.".to_owned());
        }
        if workshop_id.is_some_and(|id| id < 0) || order_id.is_some_and(|id| id < 0) {
            return Err("workshop_id and order_id must be non-negative.".to_owned());
        }

        let workshops = self.list_workshops(None, true, 0, 50)?;
        let work_orders = self.list_work_orders(None, 0, 50)?;
        let stockpiles = self.list_stockpiles(None, None, 0, 50)?;
        let source_workshops = workshops["workshops"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let source_orders = work_orders["work_orders"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let (selected_workshops, selected_orders) = filter_production_evidence(
            &source_workshops,
            &source_orders,
            search,
            workshop_id,
            order_id,
        );
        let mut reaction_codes = BTreeSet::new();
        for row in selected_workshops.iter().chain(selected_orders.iter()) {
            collect_production_reaction_codes(row, &mut reaction_codes);
        }
        let mut reaction_definitions = Vec::new();
        for reaction in reaction_codes {
            let mut detail = self.get_reaction(Some(&reaction), None, false, 0, 10)?;
            restore_reaction_lists(&mut detail);
            if let Some(rows) = detail["reactions"].as_array() {
                reaction_definitions.extend(rows.iter().cloned());
            }
        }
        let signals = production_block_signals(&selected_workshops, &selected_orders);
        let stockpile_rows = stockpiles["stockpiles"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let work_orders_total = work_orders["total_matching"].as_u64().unwrap_or(0) as usize;
        let workshops_total = workshops["total_matching"].as_u64().unwrap_or(0) as usize;
        let stockpiles_total = stockpiles["total_matching"].as_u64().unwrap_or(0) as usize;

        Ok(json!({
            "search": search,
            "workshop_id": workshop_id,
            "order_id": order_id,
            "workshops": selected_workshops,
            "work_orders": selected_orders,
            "stockpiles": stockpile_rows,
            "reaction_definitions": reaction_definitions,
            "signals": signals,
            "source_coverage": {
                "workshops": {"total_matching": workshops_total, "included": source_workshops.len(), "truncated": workshops_total > source_workshops.len()},
                "work_orders": {"total_matching": work_orders_total, "included": source_orders.len(), "truncated": work_orders_total > source_orders.len()},
                "stockpiles": {"total_matching": stockpiles_total, "included": stockpile_rows.len(), "truncated": stockpiles_total > stockpile_rows.len()}
            },
            "notes": "signals restate factual DF flags and delivery progress; they are not proof of a complete root cause. Job requirements and reaction inputs are measured in material dimensions, not item counts. Stockpile filters and directional links are evidence, not an evaluation that a specific pile can satisfy a job. Source lists are capped at 50 rows, and each workshop reports when its queued jobs or requirements are truncated."
        }))
    }

    /// Counts of the other building kinds.
    pub fn building_counts(&self) -> Result<Value, String> {
        self.call_lua_json("count buildings", "building_counts", Vec::new())
    }

    /// Which DFHack automation helpers (autofarm, tailor, autotraining...) are on.
    pub fn dfhack_automation(&self) -> Result<Value, String> {
        self.call_lua_json(
            "read DFHack automation status",
            "dfhack_automation",
            Vec::new(),
        )
    }

    /// Citizens in a strange mood: demands, deliveries, workshop and time left.
    pub fn strange_moods(&self) -> Result<Value, String> {
        self.call_lua_json("list strange moods", "strange_moods", Vec::new())
    }

    /// Returns a page of item groups (see `search_items` in the Lua module).
    pub fn search_items(
        &self,
        search: Option<&str>,
        item_type: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if !(1..=MAX_ITEM_PAGE_SIZE).contains(&limit) {
            return Err(format!(
                "The page size must be between 1 and {MAX_ITEM_PAGE_SIZE}."
            ));
        }
        let reply = self
            .with_client(
                "Could not search items (is the DFCompanion Lua module installed?)",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "search_items".to_owned(),
                        arguments: vec![
                            search.unwrap_or_default().to_owned(),
                            item_type.unwrap_or_default().to_owned(),
                            offset.to_string(),
                            limit.to_string(),
                        ],
                    })
                },
            )?
            .reply
            .value;
        let value: Value = serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the item search: {error}"))?;
        match value.get("error").and_then(Value::as_str) {
            Some(message) => Err(message.to_owned()),
            None => Ok(value),
        }
    }

    /// Returns a page of the known creatures in one group of the game's
    /// Creatures screen, optionally restricted to rows whose name, race, or
    /// profession contains `search` (case-insensitive).
    pub fn list_creatures(
        &self,
        group: CreatureGroup,
        offset: usize,
        limit: usize,
        search: Option<&str>,
    ) -> Result<Value, String> {
        if !(1..=MAX_CREATURE_PAGE_SIZE).contains(&limit) {
            return Err(format!(
                "The page size must be between 1 and {MAX_CREATURE_PAGE_SIZE}."
            ));
        }
        let creatures = self.known_creatures(None)?;
        let search = search.map(str::to_lowercase).filter(|s| !s.is_empty());
        let matching = creatures
            .rows(group)
            .iter()
            .filter(|row| search.as_deref().is_none_or(|s| matches_search(row, s)))
            .cloned()
            .collect::<Vec<_>>();
        page_result(group, &matching, offset, limit)
    }
}

/// Builds one page of `rows`, dropping trailing rows until the reply fits the
/// size limit.
fn check_page(limit: usize) -> Result<(), String> {
    if (1..=MAX_ITEM_PAGE_SIZE).contains(&limit) {
        Ok(())
    } else {
        Err(format!(
            "The page size must be between 1 and {MAX_ITEM_PAGE_SIZE}."
        ))
    }
}

fn page_result(
    group: CreatureGroup,
    rows: &[Value],
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    let total = rows.len();
    let counts_by_race = count_by_race(rows);
    let idle_count = rows.iter().filter(|row| row["idle"] == json!(true)).count();
    let mut page = rows
        .iter()
        .skip(offset)
        .take(limit)
        .map(tidy_row)
        .collect::<Vec<_>>();
    loop {
        let next_offset = offset.saturating_add(page.len());
        let mut result = json!({
            "group": group.label(),
            "total_matching": total,
            "counts_by_race": &counts_by_race,
            "offset": offset,
            "returned": page.len(),
            "creatures": &page,
        });
        if group == CreatureGroup::Residents {
            result["idle_count"] = json!(idle_count);
        }
        if next_offset < total {
            result["next_offset"] = json!(next_offset);
        }
        if result.to_string().len() <= MAX_CREATURE_REPLY_BYTES {
            return Ok(result);
        }
        if page.is_empty() {
            return Err("A creature-list response exceeded the 12 KiB size limit.".to_owned());
        }
        page.pop();
    }
}

fn is_link_failure(error: &dfhack_remote::Error) -> bool {
    matches!(
        error,
        dfhack_remote::Error::CommunicationFailure(_) | dfhack_remote::Error::ProtocolError(_)
    )
}

fn parse_lua_boolean_result(result: &str, function: &str) -> Result<bool, String> {
    match result.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!(
            "DFHack Lua function {function} returned an invalid boolean result: {other:?}"
        )),
    }
}

fn notify_version_warning(
    warning: Option<String>,
    previous_warning: &mut Option<String>,
    notifier: &Notifier,
) {
    if warning == *previous_warning {
        return;
    }
    match warning {
        Some(warning) => {
            notifier.notify(DisplayMessage::new(
                DisplayMessageKind::Warning,
                warning.clone(),
            ));
            *previous_warning = Some(warning);
        }
        None => *previous_warning = None,
    }
}

fn version_mismatch_warning(
    dwarf_fortress_version: Option<&str>,
    dfhack_version: Option<&str>,
) -> Option<String> {
    let mut differences = Vec::new();
    if let Some(actual) = dwarf_fortress_version {
        if actual != TESTED_DWARF_FORTRESS_VERSION {
            differences.push(format!(
                "Dwarf Fortress {actual} (tested with {TESTED_DWARF_FORTRESS_VERSION})"
            ));
        }
    } else {
        differences.push("Dwarf Fortress version could not be verified".to_owned());
    }
    if let Some(actual) = dfhack_version {
        if actual != TESTED_DFHACK_VERSION {
            differences.push(format!(
                "DFHack {actual} (tested with {TESTED_DFHACK_VERSION})"
            ));
        }
    } else {
        differences.push("DFHack version could not be verified".to_owned());
    }
    (!differences.is_empty()).then(|| {
        format!(
            "Version compatibility warning: {}. Continuing the read-only query; results may be incomplete or inaccurate.",
            differences.join(", ")
        )
    })
}

/// Checks a "YYYY-MM-DD" or "YYYY-MM-DD:T" game date.
pub(crate) fn validate_game_date(value: &str) -> Result<(), String> {
    let error = || {
        format!("\"{value}\" is not a game date; expected YYYY-MM-DD or YYYY-MM-DD:T (month 1-12, day 1-28).")
    };
    let (date, ticks) = match value.split_once(':') {
        Some((date, ticks)) => (date, Some(ticks)),
        None => (value, None),
    };
    let parts = date
        .split('-')
        .map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u32>().ok())
                .flatten()
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(error)?;
    let [_, month, day] = parts[..] else {
        return Err(error());
    };
    if !(1..=12).contains(&month) || !(1..=28).contains(&day) {
        return Err(error());
    }
    if let Some(ticks) = ticks {
        if ticks.parse::<u32>().map_or(true, |t| t >= 1200)
            || !ticks.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
