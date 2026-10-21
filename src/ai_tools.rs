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

use crate::game_state::{CreatureGroup, GameState, MAX_CREATURE_PAGE_SIZE, MAX_REPORTS};

pub const MAX_TOOL_CALLS_PER_TURN: usize = 3;

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
struct ReportsArgs {
    #[serde(default = "default_report_limit")]
    limit: usize,
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
                "Get a snapshot of the current game: world and save names, whether the game is paused, the current game date (year, month 1-12, day 1-28), the map size and viewed position, and how many creatures the player knows of in each group (residents, pets and livestock, other creatures, dead or missing). Start here for general questions about the fortress. This is read-only.",
                json!({}),
                |game, _: NoArgs| game.fortress_overview(),
            ),
            list_tool(
                "list_residents",
                "List the fortress's residents, like the Residents tab of the game's Creatures screen: citizens and other long-term residents with name, profession, current job, mood, squad, noble positions, age, and position. Use a unit ID with get_unit_details. Request another page with the returned next_offset when needed. Only creatures the player knows of are included. This is read-only.",
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
                "get_recent_reports",
                "Get the most recent in-game announcements and event reports (combat, moods, arrivals, and so on), oldest first. This is read-only.",
                json!({
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_REPORTS,
                        "description": "Number of most recent reports to return. Defaults to 10."
                    }
                }),
                |game, args: ReportsArgs| game.recent_reports(args.limit),
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
