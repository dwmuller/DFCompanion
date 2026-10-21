use genai::chat::{Tool, ToolName};
use serde_json::{json, Value};
use tokio::sync::broadcast;

const UNIT_LIST_TOOL_NAME: &str = "list_fortress_units";
const MAX_TOOL_CALLS_PER_TURN: usize = 3;
const MAX_UNIT_OFFSET: usize = 5_000;

pub fn definitions() -> Vec<Tool> {
    vec![Tool::new(ToolName::Custom(UNIT_LIST_TOOL_NAME.to_owned()))
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
                    "maximum": crate::game_state::MAX_UNIT_PAGE_SIZE,
                    "description": "Maximum number of units to return. Defaults to 25."
                }
            },
            "additionalProperties": false
        }))]
}

pub async fn execute(
    name: &str,
    arguments: &Value,
    notifications_tx: broadcast::Sender<String>,
) -> String {
    let result = if name == UNIT_LIST_TOOL_NAME {
        parse_unit_list_arguments(arguments).map_err(str::to_owned)
    } else {
        Err(format!("Unknown tool: {name}"))
    };

    match result {
        Ok((offset, limit)) => {
            match crate::game_state::list_units(offset, limit, notifications_tx).await {
                Ok(value) => value,
                Err(error) => json!({"error": error}),
            }
        }
        Err(error) => json!({"error": error}),
    }
    .to_string()
}

pub fn ensure_call_budget(call_count: &mut usize, requested_calls: usize) -> Result<(), String> {
    *call_count = call_count.saturating_add(requested_calls);
    if *call_count > MAX_TOOL_CALLS_PER_TURN {
        return Err(format!(
            "The model exceeded the limit of {MAX_TOOL_CALLS_PER_TURN} game-state tool calls in one turn."
        ));
    }
    Ok(())
}

fn parse_unit_list_arguments(arguments: &Value) -> Result<(usize, usize), &'static str> {
    let object = arguments
        .as_object()
        .ok_or("Tool arguments must be an object.")?;
    if object.keys().any(|key| key != "offset" && key != "limit") {
        return Err("Only offset and limit arguments are supported.");
    }
    let offset = object
        .get("offset")
        .map(|value| {
            value
                .as_u64()
                .ok_or("offset must be a non-negative integer.")
        })
        .transpose()?
        .unwrap_or(0);
    let limit = object
        .get("limit")
        .map(|value| value.as_u64().ok_or("limit must be a positive integer."))
        .transpose()?
        .unwrap_or(25);
    let offset = usize::try_from(offset).map_err(|_| "offset is too large.")?;
    let limit = usize::try_from(limit).map_err(|_| "limit is too large.")?;
    if offset > MAX_UNIT_OFFSET {
        return Err("offset exceeds the maximum supported unit-list index.");
    }
    if limit == 0 || limit > crate::game_state::MAX_UNIT_PAGE_SIZE {
        return Err("limit must be between 1 and the maximum page size.");
    }
    Ok((offset, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_list_arguments_are_bounded() {
        assert_eq!(parse_unit_list_arguments(&json!({})).unwrap(), (0, 25));
        assert_eq!(
            parse_unit_list_arguments(&json!({"offset": 100, "limit": 50})).unwrap(),
            (100, 50)
        );
        assert!(parse_unit_list_arguments(&json!({"limit": 51})).is_err());
        assert!(parse_unit_list_arguments(&json!({"offset": 5001})).is_err());
        assert!(parse_unit_list_arguments(&json!({"query": "dwarf"})).is_err());
    }

    #[test]
    fn tool_calls_are_capped_per_turn() {
        let mut count = 0;
        assert!(ensure_call_budget(&mut count, 2).is_ok());
        assert!(ensure_call_budget(&mut count, 1).is_ok());
        assert!(ensure_call_budget(&mut count, 1).is_err());
    }

    #[test]
    fn tool_definition_has_a_closed_argument_schema() {
        let definitions = definitions();
        assert_eq!(definitions.len(), 1);
        assert_eq!(
            definitions[0].name,
            ToolName::Custom(UNIT_LIST_TOOL_NAME.to_owned())
        );
        assert_eq!(
            definitions[0].schema.as_ref().unwrap()["additionalProperties"],
            false
        );
    }
}
