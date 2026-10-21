//! AI tools: schema definitions plus a thin dispatcher onto [`GameState`].
//! To add a tool, add its definition to [`definitions`] and a match arm in [`run`].

use genai::chat::{Tool, ToolCall, ToolName};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::game_state::{GameState, MAX_UNIT_PAGE_SIZE};

pub const MAX_TOOL_CALLS_PER_TURN: usize = 3;

const LIST_UNITS: &str = "list_fortress_units";
const MAX_UNIT_OFFSET: usize = 5_000;
const DEFAULT_UNIT_LIMIT: usize = 25;

pub fn definitions() -> Vec<Tool> {
    vec![Tool::new(ToolName::Custom(LIST_UNITS.to_owned()))
        .with_description(
            "List fortress units, including citizens, visitors, and other races supplied by DFHack. Returns a small page with IDs, names, race and caste numeric identifiers, soldier status, age, and position. Use the unit ID when discussing a particular person. Request another page with the returned next_offset when needed. This is read-only.",
        )
        .with_schema(json!({
            "type": "object",
            "properties": {
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_UNIT_OFFSET,
                    "description": "Zero-based index of the first unit to return."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_UNIT_PAGE_SIZE,
                    "description": "Maximum number of units to return. Defaults to 25."
                }
            },
            "additionalProperties": false
        }))]
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ListUnitsArgs {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_unit_limit")]
    limit: usize,
}

fn default_unit_limit() -> usize {
    DEFAULT_UNIT_LIMIT
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

fn parse_list_units_args(arguments: Value) -> Result<ListUnitsArgs, String> {
    let args: ListUnitsArgs = parse_args(arguments)?;
    if args.offset > MAX_UNIT_OFFSET {
        return Err("offset exceeds the maximum supported unit-list index.".to_owned());
    }
    if !(1..=MAX_UNIT_PAGE_SIZE).contains(&args.limit) {
        return Err("limit must be between 1 and the maximum page size.".to_owned());
    }
    Ok(args)
}

async fn run(game_state: &GameState, name: &str, arguments: Value) -> Result<Value, String> {
    match name {
        LIST_UNITS => {
            let args = parse_list_units_args(arguments)?;
            game_state
                .run(move |game| game.list_units(args.offset, args.limit))
                .await
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
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
        assert!(!definitions.is_empty());
        for definition in definitions {
            assert_eq!(definition.schema.unwrap()["additionalProperties"], false);
        }
    }

    #[test]
    fn unit_arguments_are_bounded() {
        let parse = |value| parse_list_units_args(value).map(|a| (a.offset, a.limit));
        assert_eq!(parse(json!({})), Ok((0, 25)));
        assert_eq!(parse(Value::Null), Ok((0, 25)));
        assert_eq!(parse(json!({"offset": 100, "limit": 50})), Ok((100, 50)));
        assert!(parse(json!({"limit": 51})).is_err());
        assert!(parse(json!({"limit": 0})).is_err());
        assert!(parse(json!({"offset": 5001})).is_err());
        assert!(parse(json!({"offset": -1})).is_err());
        assert!(parse(json!({"query": "dwarf"})).is_err());
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
