//! AI tools: a registry of typed tools that dispatch onto [`GameState`].
//!
//! To add a tool, write a `GameState` method that returns `Result<Value, String>`,
//! define a `#[derive(Deserialize)]` argument struct (if it takes arguments), and
//! add one [`tool`] entry to [`registry`]. Argument parsing, the strict schema
//! wrapper, blocking-thread dispatch, and error reporting are handled here.

use std::sync::OnceLock;

use genai::chat::{Tool, ToolCall, ToolName};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::game_state::{
    CreatureGroup, GameState, MAX_CREATURE_PAGE_SIZE, MAX_ITEM_PAGE_SIZE, MAX_REPORTS,
};

pub const MAX_TOOL_CALLS_PER_TURN: usize = 20;

const MAX_CREATURE_OFFSET: usize = 5_000;
const DEFAULT_CREATURE_LIMIT: usize = 25;
const DEFAULT_REPORT_LIMIT: usize = 10;

type Handler = Box<dyn Fn(&GameState, Value) -> Result<Value, String> + Send + Sync>;

struct ToolSpec {
    name: &'static str,
    description: &'static str,
    schema: Value,
    handler: Handler,
}

/// Builds a tool whose arguments are deserialized into `A` before `run` is called.
/// `properties` is the JSON-schema `properties` object; the wrapper makes the
/// schema strict (no additional properties).
fn tool<A: DeserializeOwned>(
    name: &'static str,
    description: &'static str,
    properties: Value,
    run: impl Fn(&GameState, A) -> Result<Value, String> + Send + Sync + 'static,
) -> ToolSpec {
    ToolSpec {
        name,
        description,
        schema: json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false,
        }),
        handler: Box::new(move |game, arguments| run(game, parse_args(arguments)?)),
    }
}

impl ToolSpec {
    /// Marks arguments as required in the schema.
    fn required(mut self, names: &[&str]) -> Self {
        self.schema["required"] = json!(names);
        self
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ListCreaturesArgs {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_creature_limit")]
    limit: usize,
    search: Option<String>,
}

fn default_creature_limit() -> usize {
    DEFAULT_CREATURE_LIMIT
}

/// A paged list of one group of the game's Creatures screen.
fn list_tool(name: &'static str, description: &'static str, group: CreatureGroup) -> ToolSpec {
    tool(
        name,
        description,
        json!({
            "offset": {
                "type": "integer",
                "minimum": 0,
                "maximum": MAX_CREATURE_OFFSET,
                "description": "Zero-based index of the first matching creature to return."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_CREATURE_PAGE_SIZE,
                "description": "Maximum number of creatures to return. Defaults to 25."
            },
            "search": {
                "type": "string",
                "description": "Only return creatures whose name, race, or profession contains this text (case-insensitive)."
            }
        }),
        move |game, args: ListCreaturesArgs| {
            if args.offset > MAX_CREATURE_OFFSET {
                return Err("offset exceeds the maximum supported list index.".to_owned());
            }
            game.list_creatures(group, args.offset, args.limit, args.search.as_deref())
        },
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitDetailsArgs {
    unit_id: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HousingArgs {
    list: Option<String>,
    z: Option<i64>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_creature_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListZonesArgs {
    #[serde(rename = "type")]
    zone_type: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_creature_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListWorkshopsArgs {
    name: Option<String>,
    #[serde(default)]
    only_with_jobs: bool,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_creature_limit")]
    limit: usize,
}

/// Adds the shared `offset` and `limit` paging properties to a schema's properties.
fn with_paging(mut properties: Value) -> Value {
    properties["offset"] = json!({
        "type": "integer",
        "minimum": 0,
        "maximum": MAX_CREATURE_OFFSET,
        "description": "Zero-based index of the first row to return."
    });
    properties["limit"] = json!({
        "type": "integer",
        "minimum": 1,
        "maximum": MAX_ITEM_PAGE_SIZE,
        "description": "Maximum number of rows to return. Defaults to 25."
    });
    properties
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchItemsArgs {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_creature_limit")]
    limit: usize,
    search: Option<String>,
    item_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportsArgs {
    #[serde(default = "default_report_limit")]
    limit: usize,
    from: Option<String>,
    to: Option<String>,
    category: Option<String>,
    since_id: Option<i64>,
    until_id: Option<i64>,
}

fn default_report_limit() -> usize {
    DEFAULT_REPORT_LIMIT
}

fn registry() -> &'static [ToolSpec] {
    static REGISTRY: OnceLock<Vec<ToolSpec>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            tool(
                "get_fortress_overview",
                "Get a snapshot of the current game: world and save names, whether the game is paused, the current game date and time (YYYY-MM-DD:T, month 1-12, day 1-28, T = ticks into the day, 0-1199), the map size and viewed position (z levels match the game's UI), and how many creatures the player knows of in each group (residents, pets and livestock, other creatures, dead or missing). Start here for general questions about the fortress. This is read-only.",
                json!({}),
                |game, _: NoArgs| game.fortress_overview(),
            ),
            list_tool(
                "list_residents",
                "List the fortress's residents, like the Residents tab of the game's Creatures screen: citizens and other long-term residents with name, profession, status (what they are doing right now: current job, else an activity such as prayer or play, else \"idle\"), job, activity, sleeping and idle flags (present only when true for sleeping/idle), mood, squad, noble positions, age, and position. The result includes idle_count, the number of idle residents among those matching. Use a unit ID with get_unit_details. Request another page with the returned next_offset when needed. Only creatures the player knows of are included. This is read-only.",
                CreatureGroup::Residents,
            ),
            list_tool(
                "list_pets_livestock",
                "List pets and livestock, like the Pets/Livestock tab of the game's Creatures screen: the fortress's tame animals with race, sex, age, training (war, hunting), grazing need, owner, availability for adoption, and pending slaughter, gelding, or training marks. Request another page with the returned next_offset when needed. This is read-only.",
                CreatureGroup::PetsAndLivestock,
            ),
            list_tool(
                "list_other_creatures",
                "List other creatures the player can currently see, like the Other tab of the game's Creatures screen: hostile units, wild animals, visitors, and merchants, each with a disposition. Creatures the player has not seen are not included. Request another page with the returned next_offset when needed. This is read-only.",
                CreatureGroup::Other,
            ),
            list_tool(
                "list_dead_missing",
                "List dead and missing creatures, like the Dead/Missing tab of the game's Creatures screen: fortress members and any visible creatures that have died, and fortress animals that are missing from the map. Each has a status of deceased or missing, and deceased creatures include a died date (year, month 1-12, day 1-28) when the game recorded one. Request another page with the returned next_offset when needed. This is read-only.",
                CreatureGroup::DeadOrMissing,
            ),
            tool(
                "get_unit_details",
                "Get details about one creature by ID (from one of the list tools): everything the list shows plus blood level, wound count, and carried item count. Creatures the player does not know of are reported as nonexistent. This is read-only.",
                json!({
                    "unit_id": {
                        "type": "integer",
                        "description": "The unit ID."
                    }
                }),
                |game, args: UnitDetailsArgs| game.unit_details(args.unit_id),
            )
            .required(&["unit_id"]),
            tool(
                "get_unit_mind",
                "Explain a resident's mood: the stress level and mood name, their recent thoughts (grouped by kind with counts and the date of the newest, e.g. \"saw dead body\", \"bedroom quality\", \"thirsty\", and unfulfilled needs), and their unmet needs (what they want: e.g. pray or meditate, eat good meal, socialize, with how unfocused it makes them, and the deity for worship needs). Use it to answer why a dwarf is unhappy. Only living citizens and residents; take the unit ID from list_residents. Thought text is derived from the game's thought types, not the game's exact wording. This is read-only.",
                json!({
                    "unit_id": {
                        "type": "integer",
                        "description": "The unit ID of a resident."
                    }
                }),
                |game, args: UnitDetailsArgs| game.unit_mind(args.unit_id),
            )
            .required(&["unit_id"]),
            tool(
                "get_food_drink_summary",
                "Summarize the fortress's food and drink stocks: total, available (not forbidden or rotten), forbidden and rotten units; a rough estimate of days of supply at the current population; the drink varieties (e.g. wine, rum) and the most plentiful food kinds; and food by category (prepared meals, meat, fish, dairy and eggs). Counts match the game's status header; edible raw plants are reported separately. The result states its consumption assumptions. Use search_items for details about a specific item or its location. This is read-only.",
                json!({}),
                |game, _: NoArgs| game.food_drink_summary(),
            ),
            tool(
                "get_housing",
                "Describe the fortress's sleeping arrangements: a summary (beds, bedroom zones and how many have an owner, citizens without an owned bedroom, free beds) with per-floor counts of beds, free beds and bedroom zones (z matches the game's UI), plus one paged list chosen by `list`: free_beds (finished beds not in an owned bedroom, with position and whether they are in a dormitory, an unassigned bedroom, or no zone) or unhoused (citizens and residents without an owned bedroom, with position). `z` restricts the list to one floor. Request another page with the returned next_offset when needed. Use it to find which floor needs beds or whether idle beds could be assigned. This is read-only.",
                with_paging(json!({
                    "list": {
                        "type": "string",
                        "enum": ["free_beds", "unhoused"],
                        "description": "Which list to return. Defaults to free_beds."
                    },
                    "z": {
                        "type": "integer",
                        "description": "Only list beds or citizens on this z level (as shown in the game's UI)."
                    }
                })),
                |game, args: HousingArgs| {
                    game.housing(args.list.as_deref().unwrap_or("free_beds"), args.z, args.offset, args.limit)
                },
            ),
            tool(
                "list_zones",
                "List the fortress's zones other than bedrooms (those are in get_housing): dining halls, dormitories, meeting halls, barracks, offices, pens, tombs, dumps and so on, each with id, name, the location it belongs to, area in tiles, centre position (z matches the game's UI) and the assigned owner or animal count. Also returns zone_types with the count and total area for every zone type including bedrooms. Use `type` to look at one kind. Request another page with the returned next_offset when needed. This is read-only.",
                with_paging(json!({
                    "type": {
                        "type": "string",
                        "description": "Only include zones whose type contains this text, e.g. \"meeting\", \"dormitory\", \"tomb\"."
                    }
                })),
                |game, args: ListZonesArgs| game.list_zones(args.zone_type.as_deref(), args.offset, args.limit),
            ),
            tool(
                "list_locations",
                "List the fortress's named locations (temples, hospitals, guildhalls, taverns, libraries) with their type, name, and the number and total area of the zones that make them up. A location with no zones cannot function yet, so this answers \"is there a working hospital or temple?\". This is read-only.",
                json!({}),
                |game, _: NoArgs| game.list_locations(),
            ),
            tool(
                "list_workshops",
                "List the fortress's workshops and furnaces individually, each with id, name, centre position (z matches the game's UI), size, whether construction is finished, the number of queued jobs, and the names of the first few jobs (e.g. a strange mood's creation job). Use `name` to look at one kind and `only_with_jobs` to see just the busy ones. Request another page with the returned next_offset when needed. This is read-only.",
                with_paging(json!({
                    "name": {
                        "type": "string",
                        "description": "Only include workshops whose name contains this text (case-insensitive), e.g. \"still\", \"smelter\"."
                    },
                    "only_with_jobs": {
                        "type": "boolean",
                        "description": "Only include workshops with queued jobs."
                    }
                })),
                |game, args: ListWorkshopsArgs| {
                    game.list_workshops(args.name.as_deref(), args.only_with_jobs, args.offset, args.limit)
                },
            ),
            tool(
                "get_building_counts",
                "Count the fortress's other buildings by kind, built and under construction: beds, tables, chairs, doors, wells, stockpiles, farm plots, statues, traps and so on. Workshops and furnaces are in list_workshops; zones in list_zones and list_locations. This is read-only.",
                json!({}),
                |game, _: NoArgs| game.building_counts(),
            ),
            tool(
                "search_items",
                "Find and count the fortress's items (stockpiled, stored in containers, or lying around; not items carried by creatures), grouped by type and description, largest quantity first: e.g. logs, drinks, food, bars, boulders, furniture. Each group shows its total quantity, how much is forbidden or rotten, and up to three sample positions (z levels match the game's UI). Also returns quantity_by_type for everything matched. Use `search` for words like \"wine\" or \"cobaltite\" and `item_type` to look at one kind of item (e.g. WOOD, DRINK, FOOD, BAR, BOULDER, CLOTH). Request another page with the returned next_offset when needed. This is read-only.",
                json!({
                    "search": {
                        "type": "string",
                        "description": "Only include items whose description or item type contains this text (case-insensitive)."
                    },
                    "item_type": {
                        "type": "string",
                        "description": "Only include items of this DF item type, e.g. WOOD, DRINK, FOOD, MEAT, PLANT, BAR, BOULDER, BLOCKS, CLOTH, WEAPON, ARMOR."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_CREATURE_OFFSET,
                        "description": "Zero-based index of the first matching group to return."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_ITEM_PAGE_SIZE,
                        "description": "Maximum number of groups to return. Defaults to 25."
                    }
                }),
                |game, args: SearchItemsArgs| {
                    if args.offset > MAX_CREATURE_OFFSET {
                        return Err("offset exceeds the maximum supported list index.".to_owned());
                    }
                    game.search_items(
                        args.search.as_deref(),
                        args.item_type.as_deref(),
                        args.offset,
                        args.limit,
                    )
                },
            ),
            tool(
                "get_recent_reports",
                "Get in-game announcements (combat, moods, arrivals, and so on) with their game dates, oldest first. Returns the newest `limit` announcements, optionally restricted to a game-date range. Each report has a unique, increasing `id`, and the result includes `latest_id`; use `since_id` (and `until_id`) to fetch exactly the reports after a known id, for example to check for new reports or to confirm none were missed. Dates are written YYYY-MM-DD or YYYY-MM-DD:T, where T is the tick within the day (0-1199); a date without ticks covers the whole day. This is read-only.",
                json!({
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_REPORTS,
                        "description": "Maximum number of reports to return (the newest ones in range). Defaults to 10."
                    },
                    "from": {
                        "type": "string",
                        "description": "Only return reports at or after this game date (inclusive), e.g. 254-06-01 or 254-06-01:600."
                    },
                    "to": {
                        "type": "string",
                        "description": "Only return reports at or before this game date (inclusive)."
                    },
                    "since_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Only return reports with an id greater than this (exclusive). When given, the OLDEST matching reports are returned, so repeating the call with the last returned id as since_id pages forward without gaps. Use the `latest_id` field of a result as the starting point for later checks."
                    },
                    "until_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Only return reports with an id up to and including this one."
                    },
                    "category": {
                        "type": "string",
                        "enum": ["death", "combat", "mood", "arrivals", "jobs", "mining", "population", "environment", "other"],
                        "description": "Only return reports of this category: death (citizen/pet deaths), combat (ambushes, sieges, attacks, thefts), mood (strange moods, berserk, stress), arrivals (migrants, caravans, visitors), jobs (cancelled jobs, quotas, masterworks, artifacts), mining (ore and gem strikes, discoveries), population (births, military and noble changes), environment (weather, seasons)."
                    }
                }),
                |game, args: ReportsArgs| {
                    game.recent_reports(
                        args.limit,
                        args.from.as_deref(),
                        args.to.as_deref(),
                        args.category.as_deref(),
                        args.since_id,
                        args.until_id,
                    )
                },
            ),
        ]
    })
}

pub fn definitions() -> Vec<Tool> {
    registry()
        .iter()
        .map(|spec| {
            Tool::new(ToolName::Custom(spec.name.to_owned()))
                .with_description(spec.description)
                .with_schema(spec.schema.clone())
        })
        .collect()
}

fn is_required(spec: &ToolSpec, argument: &str) -> bool {
    spec.schema["required"]
        .as_array()
        .is_some_and(|required| required.iter().any(|n| n.as_str() == Some(argument)))
}

/// Concise tool list for the `/tools` command: names with their argument names,
/// optional arguments in brackets.
pub fn describe_tools() -> String {
    let mut text = String::from("Available tools (optional arguments in brackets; /tool <name> for details):");
    for spec in registry() {
        let arguments = spec.schema["properties"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, _)| {
                if is_required(spec, name) {
                    name.clone()
                } else {
                    format!("[{name}]")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!("\n{}({arguments})", spec.name));
    }
    text
}

/// Full description of one tool for the `/tool` command.
pub fn describe_tool(name: &str) -> Result<String, String> {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| format!("Unknown tool: {name}. Use /tools to list the tools."))?;
    let mut text = format!("{}\n{}", spec.name, spec.description);
    match spec.schema["properties"].as_object().filter(|p| !p.is_empty()) {
        None => text.push_str("\nArguments: none"),
        Some(properties) => {
            text.push_str("\nArguments:");
            for (argument, property) in properties {
                text.push_str(&format!(
                    "\n  {argument} ({}, {})",
                    property["type"].as_str().unwrap_or("any"),
                    if is_required(spec, argument) { "required" } else { "optional" }
                ));
                match (property.get("minimum"), property.get("maximum")) {
                    (Some(min), Some(max)) => text.push_str(&format!(" [{min}..{max}]")),
                    (None, Some(max)) => text.push_str(&format!(" [max {max}]")),
                    (Some(min), None) => text.push_str(&format!(" [min {min}]")),
                    (None, None) => {}
                }
                if let Some(description) = property["description"].as_str() {
                    text.push_str(&format!(": {description}"));
                }
            }
        }
    }
    Ok(text)
}
/// Deserializes tool arguments into a typed struct. Argument structs should use
/// `#[serde(deny_unknown_fields)]` so unexpected arguments are rejected.
fn parse_args<T: DeserializeOwned>(arguments: Value) -> Result<T, String> {
    // Some providers send no arguments as null instead of an empty object.
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    serde_json::from_value(arguments).map_err(|error| format!("Invalid tool arguments: {error}"))
}

async fn run(game_state: &GameState, name: &str, arguments: Value) -> Result<Value, String> {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| format!("Unknown tool: {name}"))?;
    game_state
        .run(move |game| (spec.handler)(game, arguments))
        .await
}

/// Runs a tool call and returns the JSON text to send back to the model.
/// Failures are reported as `{"error": ...}` so the model can recover.
pub async fn execute(game_state: &GameState, call: &ToolCall) -> String {
    execute_named(game_state, &call.fn_name, call.fn_arguments.clone()).await
}

pub async fn execute_named(game_state: &GameState, name: &str, arguments: Value) -> String {
    run(game_state, name, arguments)
        .await
        .unwrap_or_else(|error| json!({"error": error}))
        .to_string()
}

/// Per-turn limit on tool calls.
pub struct ToolBudget {
    remaining: usize,
}

impl ToolBudget {
    pub fn new() -> Self {
        Self {
            remaining: MAX_TOOL_CALLS_PER_TURN,
        }
    }

    /// Consumes one call. When exhausted, returns the message to give the model
    /// instead of running the tool.
    pub fn take(&mut self) -> Result<(), String> {
        if self.remaining == 0 {
            return Err(format!(
                "Tool call limit of {MAX_TOOL_CALLS_PER_TURN} per turn reached. Answer with the information already gathered."
            ));
        }
        self.remaining -= 1;
        Ok(())
    }
}

impl Default for ToolBudget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_state() -> GameState {
        GameState::new(tokio::sync::broadcast::channel(1).0)
    }

    #[tokio::test]
    async fn unknown_tools_are_reported_as_errors() {
        let result = execute_named(&game_state(), "nope", json!({})).await;
        assert!(result.contains("Unknown tool: nope"));
    }

    #[test]
    fn definitions_have_strict_schemas() {
        let definitions = definitions();
        assert!(definitions.len() >= 7);
        for definition in definitions {
            assert_eq!(definition.schema.unwrap()["additionalProperties"], false);
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut names = registry().iter().map(|s| s.name).collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), registry().len());
    }

    #[tokio::test]
    async fn bad_arguments_are_rejected_before_touching_the_game() {
        let game = game_state();
        for (name, args) in [
            ("list_residents", json!({"offset": 5001})),
            ("list_pets_livestock", json!({"limit": 0})),
            ("list_other_creatures", json!({"limit": 51})),
            ("list_dead_missing", json!({"offset": -1})),
            ("list_residents", json!({"query": "dwarf"})),
            ("get_unit_details", json!({})),
            ("get_fortress_overview", json!({"x": 1})),
            ("get_recent_reports", json!({"limit": 0})),
        ] {
            let result = execute_named(&game, name, args.clone()).await;
            let error = serde_json::from_str::<Value>(&result).unwrap()["error"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            assert!(
                !error.is_empty() && !error.contains("DFHack"),
                "{name} {args}: {error}"
            );
        }
    }

    #[test]
    fn tool_descriptions_list_arguments() {
        let list = describe_tools();
        for spec in registry() {
            assert!(list.contains(spec.name));
        }
        assert!(list.contains("get_unit_details(unit_id)"));
        assert!(list.contains("list_residents([offset], [limit], [search])"));
        assert!(list.contains("get_fortress_overview()"));

        let text = describe_tool("get_unit_details").unwrap();
        assert!(text.contains("unit_id (integer, required)"));
        let text = describe_tool("list_residents").unwrap();
        assert!(text.contains("limit (integer, optional) [1..50]"));
        assert!(describe_tool("get_fortress_overview").unwrap().contains("Arguments: none"));
        assert!(describe_tool("nope").is_err());
    }

    #[test]
    fn list_creatures_defaults() {
        let args: ListCreaturesArgs = parse_args(Value::Null).unwrap();
        assert_eq!(
            args,
            ListCreaturesArgs {
                offset: 0,
                limit: 25,
                search: None
            }
        );
    }

    #[test]
    fn budget_is_capped_per_turn() {
        let mut budget = ToolBudget::new();
        for _ in 0..MAX_TOOL_CALLS_PER_TURN {
            assert!(budget.take().is_ok());
        }
        assert!(budget.take().is_err());
    }
}
