use super::*;
use crate::game_state::{LifecycleState, LifecycleStatus};
use crate::notifications::{NotificationHistory, Notifier};

#[test]
fn should_read_staged_note_changes_within_the_same_turn() {
    let allocator = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
    let mut notes = NoteWorkspace::new(crate::notes::Notes::default(), allocator);
    execute_note(&mut notes, "create_note", json!({"text": "first"})).unwrap();
    execute_note(
        &mut notes,
        "edit_note",
        json!({"id": "n1", "text": "second"}),
    )
    .unwrap();

    let result = execute_note(&mut notes, "read_note", json!({"id": "n1"})).unwrap();

    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap()["text"],
        "second"
    );
}

#[test]
fn should_reject_note_tool_changes_to_player_notes() {
    use crate::notes::{Note, NoteSource, Notes};
    let notes = Notes::from_records(vec![Note::new(
        "n1".into(),
        "player".into(),
        NoteSource::Player,
    )
    .unwrap()])
    .unwrap();
    let mut workspace = NoteWorkspace::new(
        notes,
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(2)),
    );

    let result = execute_note(&mut workspace, "delete_note", json!({"id": "n1"})).unwrap();

    assert!(serde_json::from_str::<Value>(&result).unwrap()["error"].is_string());
}

/// A `GameState` with the given loading state. The watch sender is dropped,
/// but a receiver still reads back the last value it held.
fn game_state_with(state: LifecycleState) -> GameState {
    let lifecycle = tokio::sync::watch::channel(LifecycleStatus {
        state,
        fortress_name: None,
        session_id: None,
    })
    .1;
    GameState::new(Notifier::new(NotificationHistory::new()), lifecycle)
}

fn game_state() -> GameState {
    game_state_with(LifecycleState::MapLoaded)
}

#[tokio::test]
async fn unknown_tools_are_reported_as_errors() {
    let result = execute_named(&game_state(), "nope", json!({})).await;
    assert!(result.contains("Unknown tool: nope"));
}

#[tokio::test]
async fn tools_are_gated_until_a_fortress_is_loaded() {
    for state in [
        LifecycleState::Unreachable,
        LifecycleState::Initialized,
        LifecycleState::WorldLoaded,
    ] {
        let result =
            execute_named(&game_state_with(state), "get_fortress_overview", json!({})).await;
        assert_eq!(
            serde_json::from_str::<Value>(&result).unwrap()["error"],
            NO_FORTRESS_LOADED,
            "a tool call with state {state:?} should be gated"
        );
    }
}

#[tokio::test]
async fn bad_arguments_are_reported_before_the_fortress_gate() {
    // A bad argument still gets its own error even with no fortress loaded,
    // because arguments are validated before the gate.
    let game = game_state_with(LifecycleState::Unreachable);
    let result = execute_named(&game, "list_residents", json!({"query": "dwarf"})).await;
    let error = serde_json::from_str::<Value>(&result).unwrap()["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(!error.is_empty() && error != NO_FORTRESS_LOADED, "{error}");
}

#[test]
fn definitions_have_strict_schemas() {
    let definitions = definitions();
    assert!(definitions.len() >= 8);
    for definition in definitions {
        assert_eq!(definition.schema.unwrap()["additionalProperties"], false);
    }
}

#[test]
fn exposed_definitions_should_use_short_descriptions_and_offer_help() {
    let definitions = definitions();
    let work_orders = definitions
        .iter()
        .find(|definition| definition.name == ToolName::Custom("list_work_orders".to_owned()))
        .unwrap();
    let help = definitions
        .iter()
        .find(|definition| definition.name == ToolName::Custom("tool_help".to_owned()))
        .unwrap();

    assert!(work_orders.description.as_deref().unwrap().len() < 200);
    assert!(help.schema.as_ref().unwrap()["required"] == json!(["name"]));
}

#[test]
fn should_explain_internal_production_ids_in_exposed_tool_descriptions() {
    let definitions = definitions();
    for name in [
        "list_work_orders",
        "list_stockpiles",
        "list_workshops",
        "diagnose_production_block",
    ] {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == ToolName::Custom(name.to_owned()))
            .unwrap();
        let description = definition.description.as_deref().unwrap();
        assert!(description.contains("IDs are internal"), "{name}");
        assert!(description.contains("players"), "{name}");
    }
}

#[tokio::test]
async fn tool_help_should_work_without_a_loaded_fortress() {
    let result = execute_named(
        &game_state_with(LifecycleState::Unreachable),
        "tool_help",
        json!({"name": "list_work_orders"}),
    )
    .await;
    let result = serde_json::from_str::<Value>(&result).unwrap();

    assert_eq!(result["tool"], "list_work_orders");
    assert!(result["help"]
        .as_str()
        .unwrap()
        .contains("resident's current job/status"));
}

#[tokio::test]
async fn tool_help_should_reject_unknown_tools() {
    let result = execute_named(&game_state(), "tool_help", json!({"name": "unknown"})).await;

    assert!(serde_json::from_str::<Value>(&result).unwrap()["error"]
        .as_str()
        .unwrap()
        .contains("Unknown tool"));
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
        ("list_work_details", json!({"offset": 5001})),
        ("list_work_details", json!({"unit_id": -1})),
        ("list_work_details", json!({"unrecognized": true})),
        ("list_work_details", json!({"labor": 5})),
        ("get_labor_coverage", json!({})),
        ("get_labor_coverage", json!({"labor": ""})),
        (
            "get_labor_coverage",
            json!({"labor": "mine", "idle_only": "yes"}),
        ),
        ("list_work_orders", json!({"offset": 5001})),
        ("list_work_orders", json!({"limit": 0})),
        ("list_work_orders", json!({"search": 1})),
        ("list_work_orders", json!({"unrecognized": true})),
        ("get_reaction", json!({})),
        (
            "get_reaction",
            json!({"code": "PIG_IRON_MAKING", "search": "pig iron"}),
        ),
        ("get_reaction", json!({"search": "iron", "limit": 0})),
        (
            "get_reaction",
            json!({"code": "iron", "unrecognized": true}),
        ),
        ("get_floor_plan", json!({})),
        ("get_floor_plan", json!({"z": 0, "width": 97})),
        ("get_floor_plan", json!({"z": 0, "height": 0})),
        ("inspect_tile", json!({"x": 1, "y": 1})),
        ("inspect_tile", json!({"x": -1, "y": 1, "z": 1})),
        (
            "inspect_tile",
            json!({"x": 1, "y": 1, "z": 1, "unknown": true}),
        ),
        ("find_candidates", json!({"limit": 26})),
        ("find_candidates", json!({"skill_class": "combat"})),
        ("find_candidates", json!({"unknown": true})),
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
    assert!(
        list.contains("get_unit_details(unit_ids, [include], [skill_class], [min_skill_level])")
    );
    assert!(list.contains("list_residents([offset], [limit], [search])"));
    assert!(list.contains("get_fortress_overview()"));
    assert!(list.contains("get_fortress_geometry()"));
    assert!(list.contains("get_floor_plan(z, [x], [y], [width], [height])"));
    assert!(list.contains("list_work_details"));
    assert!(list.contains("get_labor_coverage(labor, [idle_only], [offset], [limit])"));
    assert!(list.contains("list_work_orders"));

    let text = describe_tool("get_unit_details").unwrap();
    assert!(text.contains("unit_ids (array, required)"));
    assert!(text.contains("attributes"));
    assert!(text.contains("inventory"));
    assert!(text.contains("work"));
    assert!(text.contains("specialized"));
    assert!(text.contains("native labor toggles"));
    assert!(text.contains("historical provenance"));
    let text = describe_tool("list_work_details").unwrap();
    assert!(text.contains("search"));
    assert!(text.contains("labor"));
    assert!(text.contains("unit_id"));
    assert!(text.contains("assignment_mode"));
    assert!(text.contains("selected_workers"));
    assert!(text.contains("specialized"));
    assert!(describe_tool("list_stockpiles")
        .unwrap()
        .contains("links-only mode"));
    assert!(describe_tool("list_workshops")
        .unwrap()
        .contains("stockpile/workshop links"));
    assert!(describe_tool("list_workshops")
        .unwrap()
        .contains("wait_reasons"));
    assert!(describe_tool("diagnose_production_block")
        .unwrap()
        .contains("workshop_id"));
    let text = describe_tool("get_labor_coverage").unwrap();
    assert!(text.contains("labor (string, required)"));
    assert!(text.contains("idle_only"));
    let text = describe_tool("list_work_orders").unwrap();
    assert!(text.contains("search"));
    assert!(text.contains("offset"));
    assert!(text.contains("limit"));
    let description = &registry()
        .iter()
        .find(|spec| spec.name == "list_work_orders")
        .unwrap()
        .description;
    assert!(description
        .contains("Prefer this to determine whether production has already been ordered"));
    assert!(description.contains("list_workshops"));
    assert!(description.contains("resident's current job/status"));
    assert!(description.contains("labor/work-detail assignments"));
    let text = describe_tool("list_residents").unwrap();
    assert!(text.contains("limit (integer, optional) [1..50]"));
    assert!(describe_tool("get_fortress_overview")
        .unwrap()
        .contains("Arguments: none"));
    let text = describe_tool("get_floor_plan").unwrap();
    assert!(text.contains("z (integer, required)"));
    assert!(text.contains("hidden and unavailable tiles are '?'"));
    let text = describe_tool("inspect_tile").unwrap();
    assert!(text.contains("x (integer, required)"));
    assert!(text.contains("Hidden tiles return only a redaction note"));
    assert!(describe_tool("nope").is_err());
}

#[test]
fn short_descriptions_should_reduce_initial_tool_context() {
    let detailed_description_tokens = registry()
        .iter()
        .map(|spec| tokenx_rs::estimate_token_count(spec.description))
        .sum::<usize>();
    let exposed_description_tokens = definitions()
        .iter()
        .filter_map(|definition| definition.description.as_deref())
        .map(tokenx_rs::estimate_token_count)
        .sum::<usize>();

    assert!(exposed_description_tokens < detailed_description_tokens / 2);
}

#[test]
fn get_unit_details_schema_advertises_inventory() {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == "get_unit_details")
        .unwrap();
    let include_parts = spec.schema["properties"]["include"]["items"]["enum"]
        .as_array()
        .unwrap();
    assert!(include_parts.contains(&json!("inventory")));
}

#[test]
fn get_unit_details_schema_advertises_work() {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == "get_unit_details")
        .unwrap();
    let include_parts = spec.schema["properties"]["include"]["items"]["enum"]
        .as_array()
        .unwrap();
    assert!(include_parts.contains(&json!("work")));
}

#[test]
fn get_unit_details_schema_advertises_relations() {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == "get_unit_details")
        .unwrap();
    let include_parts = spec.schema["properties"]["include"]["items"]["enum"]
        .as_array()
        .unwrap();
    assert!(include_parts.contains(&json!("relations")));
}

#[test]
fn list_work_details_schema_is_strict_and_has_expected_arguments() {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == "list_work_details")
        .unwrap();
    assert_eq!(spec.schema["additionalProperties"], false);
    assert_eq!(spec.schema["properties"]["unit_id"]["minimum"], 0);
    assert!(spec.schema["properties"].get("search").is_some());
    assert!(spec.schema["properties"].get("labor").is_some());
    assert!(spec.schema["properties"].get("offset").is_some());
    assert!(spec.schema["properties"].get("limit").is_some());
}

#[test]
fn list_work_orders_schema_is_strict_and_has_expected_arguments() {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == "list_work_orders")
        .unwrap();
    assert_eq!(spec.schema["additionalProperties"], false);
    assert_eq!(
        spec.schema["properties"]["offset"]["maximum"],
        MAX_WORK_ORDER_OFFSET
    );
    assert_eq!(spec.schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(
        spec.schema["properties"]["limit"]["maximum"],
        MAX_ITEM_PAGE_SIZE
    );
    assert!(spec.schema["properties"].get("search").is_some());

    let args: ListWorkOrdersArgs = parse_args(json!({})).unwrap();
    assert_eq!(
        args,
        ListWorkOrdersArgs {
            search: None,
            offset: 0,
            limit: DEFAULT_CREATURE_LIMIT,
        }
    );
}

#[test]
fn get_reaction_schema_is_strict_and_defaults_to_available_reactions() {
    let spec = registry()
        .iter()
        .find(|spec| spec.name == "get_reaction")
        .unwrap();
    assert_eq!(spec.schema["additionalProperties"], false);
    assert_eq!(
        spec.schema["properties"]["offset"]["maximum"],
        MAX_REACTION_OFFSET
    );
    assert_eq!(spec.schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(
        spec.schema["properties"]["limit"]["maximum"],
        MAX_ITEM_PAGE_SIZE
    );
    assert!(spec.schema["properties"].get("code").is_some());
    assert!(spec.schema["properties"].get("search").is_some());

    let args: GetReactionArgs = parse_args(json!({})).unwrap();
    assert_eq!(
        args,
        GetReactionArgs {
            code: None,
            search: None,
            available_only: true,
            offset: 0,
            limit: DEFAULT_REACTION_LIMIT,
        }
    );
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
