//! Connection to Dwarf Fortress through DFHack's RemoteFortressReader.
//!
//! [`GameState`] owns one cached DFHack connection. The version check runs only
//! when a connection is (re)established. Query methods validate their arguments,
//! make synchronous DFHack calls, and shape the data for the AI. They block, so
//! async callers should use [`GameState::run`].
//!
//! The AI should know only what the player knows. Creature information therefore
//! comes from the `rpc.dfcompanion` Lua module (see `lua/rpc/dfcompanion.lua`),
//! which lists the creatures the player is aware of, grouped like the game's
//! Creatures screen. Queries fail rather than fall back to the unfiltered unit
//! list when that module is unavailable.

use dfhack_remote::{Client, CoreRunLuaRequest, UnitDefinition};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

const TESTED_DWARF_FORTRESS_VERSION: &str = "53.16";
const TESTED_DFHACK_VERSION: &str = "53.16-r2";
pub const MAX_CREATURE_PAGE_SIZE: usize = 50;
pub const MAX_REPORTS: usize = 50;
pub const MAX_ITEM_PAGE_SIZE: usize = 50;
const MAX_CREATURE_REPLY_BYTES: usize = 12 * 1024;
const LUA_MODULE: &str = "rpc.dfcompanion";

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
        let mut parsed: Value = serde_json::from_str(json_text)
            .map_err(|error| format!("The DFCompanion Lua module returned invalid data: {error}"))?;
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

fn count_by_race(rows: &[Value]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(text_field(row, "race").to_owned()).or_default() += 1;
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

/// Cheaply cloneable handle to the shared DFHack connection.
#[derive(Clone)]
pub struct GameState {
    connection: Arc<Mutex<Connection>>,
    notifications_tx: tokio::sync::broadcast::Sender<String>,
}

impl GameState {
    pub fn new(notifications_tx: tokio::sync::broadcast::Sender<String>) -> Self {
        Self {
            connection: Arc::default(),
            notifications_tx,
        }
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
            &self.notifications_tx,
        );
        connection.client = Some(client);
        Ok(())
    }

    fn unit_list(&self) -> Result<Vec<UnitDefinition>, String> {
        Ok(self
            .with_client("Could not retrieve the fortress unit list", |client| {
                client.remote_fortress_reader().get_unit_list()
            })?
            .reply
            .creature_list)
    }

    fn known_creatures(&self) -> Result<KnownCreatures, String> {
        let reply = self
            .with_client(
                "Could not determine which creatures the player knows of (is the DFCompanion Lua module installed?)",
                |client| {
                    client.core().run_lua(CoreRunLuaRequest {
                        module: LUA_MODULE.to_owned(),
                        function: "known_creatures".to_owned(),
                        arguments: Vec::new(),
                    })
                },
            )?
            .reply
            .value;
        KnownCreatures::parse(&reply.concat())
    }

    fn game_date(&self) -> Result<String, String> {
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
        let name = if name.is_empty() { "(no fortress loaded)".to_owned() } else { name };
        Ok(format!(
            "Connected to Dwarf Fortress. Fortress: {name}. Date: {date}. Game is {}.",
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
        let creatures = self.known_creatures()?;
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

    /// Returns detailed information about one creature the player knows of.
    /// Creatures the player does not know of are reported as nonexistent.
    pub fn unit_details(&self, unit_id: i32) -> Result<Value, String> {
        let creatures = self.known_creatures()?;
        let (group, row) = creatures
            .find(i64::from(unit_id))
            .ok_or_else(|| format!("No known creature with ID {unit_id}."))?;
        let mut details = tidy_row(row);
        details["group"] = json!(group.label());
        if let Some(unit) = self.unit_list()?.iter().find(|unit| unit.id == unit_id) {
            details["blood_percent"] = json!(unit
                .blood_max
                .zip(unit.blood_count)
                .filter(|(max, _)| *max > 0)
                .map(|(max, count)| count * 100 / max));
            details["wound_count"] = json!(unit.wounds.len());
            details["inventory_item_count"] = json!(unit.inventory.len());
            details["rider_id"] = json!(unit.rider_id.filter(|id| *id >= 0));
        }
        Ok(details)
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
            return Err(format!("The report limit must be between 1 and {MAX_REPORTS}."));
        }
        for (name, value) in [("from", from), ("to", to)] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                validate_game_date(value).map_err(|error| format!("Invalid {name}: {error}"))?;
            }
        }
        let reply = self
            .with_client("Could not retrieve game reports (is the DFCompanion Lua module installed?)", |client| {
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
            })?
            .reply
            .value;
        serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the game reports: {error}"))
    }

    /// Returns a resident's mood, stress, recent thoughts and unmet needs (see
    /// `unit_mind` in the Lua module).
    pub fn unit_mind(&self, unit_id: i32) -> Result<Value, String> {
        let reply = self
            .with_client("Could not read the unit's mind (is the DFCompanion Lua module installed?)", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "unit_mind".to_owned(),
                    arguments: vec![unit_id.to_string()],
                })
            })?
            .reply
            .value;
        let value: Value = serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the unit's mind: {error}"))?;
        match value.get("error").and_then(Value::as_str) {
            Some(message) => Err(message.to_owned()),
            None => Ok(value),
        }
    }

    /// Returns food and drink stocks with rough days of supply (see
    /// `food_drink_summary` in the Lua module).
    pub fn food_drink_summary(&self) -> Result<Value, String> {
        let reply = self
            .with_client("Could not summarize food and drink (is the DFCompanion Lua module installed?)", |client| {
                client.core().run_lua(CoreRunLuaRequest {
                    module: LUA_MODULE.to_owned(),
                    function: "food_drink_summary".to_owned(),
                    arguments: Vec::new(),
                })
            })?
            .reply
            .value;
        serde_json::from_str(&reply.concat())
            .map_err(|error| format!("Could not parse the food and drink summary: {error}"))
    }

    /// Calls a Lua function in the DFCompanion module with string arguments and
    /// parses its JSON reply, turning an `error` field into `Err`.
    fn call_lua_json(&self, what: &str, function: &str, arguments: Vec<String>) -> Result<Value, String> {
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

    /// Housing summary with one paged list (see `housing` in the Lua module).
    pub fn housing(&self, list: &str, z: Option<i64>, offset: usize, limit: usize) -> Result<Value, String> {
        check_page(limit)?;
        self.call_lua_json(
            "describe housing",
            "housing",
            vec![list.to_owned(), z.map(|z| z.to_string()).unwrap_or_default(), offset.to_string(), limit.to_string()],
        )
    }

    /// A page of non-bedroom zones (see `list_zones` in the Lua module).
    pub fn list_zones(&self, zone_type: Option<&str>, offset: usize, limit: usize) -> Result<Value, String> {
        check_page(limit)?;
        self.call_lua_json(
            "list zones",
            "list_zones",
            vec![zone_type.unwrap_or_default().to_owned(), offset.to_string(), limit.to_string()],
        )
    }

    /// Named locations such as temples and hospitals.
    pub fn list_locations(&self) -> Result<Value, String> {
        self.call_lua_json("list locations", "list_locations", Vec::new())
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
        self.call_lua_json(
            "list workshops",
            "list_workshops",
            vec![
                name.unwrap_or_default().to_owned(),
                only_with_jobs.to_string(),
                offset.to_string(),
                limit.to_string(),
            ],
        )
    }

    /// Counts of the other building kinds.
    pub fn building_counts(&self) -> Result<Value, String> {
        self.call_lua_json("count buildings", "building_counts", Vec::new())
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
            return Err(format!("The page size must be between 1 and {MAX_ITEM_PAGE_SIZE}."));
        }
        let reply = self
            .with_client("Could not search items (is the DFCompanion Lua module installed?)", |client| {
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
            })?
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
        let creatures = self.known_creatures()?;
        let search = search.map(str::to_lowercase).filter(|s| !s.is_empty());
        let matching = creatures
            .rows(group)
            .iter()
            .filter(|row| search.as_deref().is_none_or(|s| matches_search(row, s)))
            .cloned()
            .collect::<Vec<_>>();
        Ok(page_result(group, &matching, offset, limit)?)
    }
}

/// Builds one page of `rows`, dropping trailing rows until the reply fits the
/// size limit.
fn check_page(limit: usize) -> Result<(), String> {
    if (1..=MAX_ITEM_PAGE_SIZE).contains(&limit) {
        Ok(())
    } else {
        Err(format!("The page size must be between 1 and {MAX_ITEM_PAGE_SIZE}."))
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

fn notify_version_warning(
    warning: Option<String>,
    previous_warning: &mut Option<String>,
    notifications_tx: &tokio::sync::broadcast::Sender<String>,
) {
    if warning == *previous_warning {
        return;
    }
    match warning {
        Some(warning) => {
            if notifications_tx.send(warning.clone()).is_ok() {
                *previous_warning = Some(warning);
            }
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
    let error = || format!("\"{value}\" is not a game date; expected YYYY-MM-DD or YYYY-MM-DD:T (month 1-12, day 1-28).");
    let (date, ticks) = match value.split_once(':') {
        Some((date, ticks)) => (date, Some(ticks)),
        None => (value, None),
    };
    let parts = date
        .split('-')
        .map(|p| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse::<u32>().ok()).flatten())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(error)?;
    let [_, month, day] = parts[..] else {
        return Err(error());
    };
    if !(1..=12).contains(&month) || !(1..=28).contains(&day) {
        return Err(error());
    }
    if let Some(ticks) = ticks {
        if ticks.parse::<u32>().map_or(true, |t| t >= 1200) || !ticks.bytes().all(|b| b.is_ascii_digit()) {
            return Err(error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_dates_are_validated() {
        for ok in ["254-07-05", "254-7-5:0", "1-12-28:1199"] {
            assert!(validate_game_date(ok).is_ok(), "{ok}");
        }
        for bad in ["", "254-13-01", "254-01-29", "254-01", "254-01-01:1200", "254-01-01:x", "a-b-c", "-1-01-01"] {
            assert!(validate_game_date(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn version_preflight_warns_when_either_version_differs() {
        assert_eq!(
            version_mismatch_warning(Some("53.16"), Some("53.16-r2")),
            None
        );
        assert!(version_mismatch_warning(Some("53.17"), Some("53.16-r2"))
            .unwrap()
            .contains("Dwarf Fortress 53.17"));
        assert!(version_mismatch_warning(Some("53.16"), Some("53.17-r1"))
            .unwrap()
            .contains("DFHack 53.17-r1"));
        assert!(version_mismatch_warning(None, None)
            .unwrap()
            .contains("could not be verified"));
    }

    #[test]
    fn version_warning_is_notified_only_when_it_changes() {
        let (notifications_tx, mut notifications_rx) = tokio::sync::broadcast::channel(4);
        let first_warning = Some("first warning".to_owned());
        let mut previous_warning = None;

        notify_version_warning(
            first_warning.clone(),
            &mut previous_warning,
            &notifications_tx,
        );
        assert_eq!(notifications_rx.try_recv().unwrap(), "first warning");

        notify_version_warning(
            first_warning.clone(),
            &mut previous_warning,
            &notifications_tx,
        );
        assert!(notifications_rx.try_recv().is_err());

        let changed_warning = Some("changed warning".to_owned());
        notify_version_warning(changed_warning, &mut previous_warning, &notifications_tx);
        assert_eq!(notifications_rx.try_recv().unwrap(), "changed warning");

        notify_version_warning(None, &mut previous_warning, &notifications_tx);
        notify_version_warning(first_warning, &mut previous_warning, &notifications_tx);
        assert_eq!(notifications_rx.try_recv().unwrap(), "first warning");
    }
}
