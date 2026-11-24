use crate::notifications::NotificationHistory;

use super::*;

#[test]
fn game_dates_are_validated() {
    for ok in ["254-07-05", "254-7-5:0", "1-12-28:1199"] {
        assert!(validate_game_date(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "254-13-01",
        "254-01-29",
        "254-01",
        "254-01-01:1200",
        "254-01-01:x",
        "a-b-c",
        "-1-01-01",
    ] {
        assert!(validate_game_date(bad).is_err(), "{bad}");
    }
}

#[test]
fn should_parse_lifecycle_snapshot_when_fortress_site_is_loaded() {
    let snapshot: LifecycleSnapshot = serde_json::from_value(json!({
        "world_loaded": true,
        "map_loaded": true,
        "fortress_name": "Bridgedmine",
        "site_id": 41
    }))
    .unwrap();

    assert_eq!(snapshot.site_id, Some(41));
}

#[test]
fn should_parse_lifecycle_snapshot_when_no_fortress_site_is_loaded() {
    let snapshot: LifecycleSnapshot = serde_json::from_value(json!({
        "world_loaded": true,
        "map_loaded": false,
        "fortress_name": ""
    }))
    .unwrap();

    assert_eq!(snapshot.site_id, None);
}

#[test]
fn empty_detail_lists_are_restored() {
    let mut details = json!({
        "skills": {},
        "mind": {"recent_thoughts": {}, "unmet_needs": [1]},
        "inventory": {"items": {}},
        "work": {"work_details": {}}
    });
    restore_empty_lists(&mut details);
    assert_eq!(details["skills"], json!([]));
    assert_eq!(details["mind"]["recent_thoughts"], json!([]));
    assert_eq!(details["mind"]["unmet_needs"], json!([1]));
    assert_eq!(details["inventory"]["items"], json!([]));
    assert_eq!(details["work"]["work_details"], json!([]));
    let mut plain = json!({"id": 1});
    restore_empty_lists(&mut plain);
    assert_eq!(plain, json!({"id": 1}));
}

#[test]
fn inventory_is_a_supported_detail_part_and_unknown_parts_are_rejected() {
    assert!(validate_detail_parts(&["inventory".to_owned()]).is_ok());
    let error = validate_detail_parts(&["unknown".to_owned()]).unwrap_err();
    assert!(error.contains("Unknown part \"unknown\""));
}

#[test]
fn work_is_a_supported_detail_part_and_unknown_parts_are_rejected() {
    assert!(validate_detail_parts(&["work".to_owned()]).is_ok());
    let error = validate_detail_parts(&["unknown".to_owned()]).unwrap_err();
    assert!(error.contains("Unknown part \"unknown\""));
}

#[test]
fn empty_labor_relationship_lists_are_restored() {
    let mut details = json!({
        "work": {
            "enabled_labors": [
                {"name": "mine", "details": {}}
            ],
            "active_automation": {}
        }
    });

    restore_empty_lists(&mut details);

    assert_eq!(details["work"]["active_automation"], json!([]));
    assert_eq!(details["work"]["enabled_labors"][0]["details"], json!([]));
}

#[test]
fn labor_coverage_normalizes_empty_relationship_lists() {
    let mut result = json!({
        "covering_details": {},
        "residents": [
            {"id": 1, "applicable_details": {}}
        ]
    });
    restore_empty_list(&mut result["covering_details"]);
    restore_empty_list(&mut result["residents"]);
    for resident in result["residents"].as_array_mut().unwrap() {
        restore_empty_list(&mut resident["applicable_details"]);
    }

    assert_eq!(result["covering_details"], json!([]));
    assert_eq!(result["residents"][0]["applicable_details"], json!([]));
}

#[test]
fn empty_work_detail_lists_are_restored() {
    let mut result = json!({"work_details": {}, "notes": "unchanged"});
    restore_work_detail_lists(&mut result);
    assert_eq!(result["work_details"], json!([]));
    assert_eq!(result["notes"], "unchanged");

    let mut details = json!({
        "work": {
            "work_details": [
                {"labors": {}, "explicit_member": false},
                {"labors": ["mining"]}
            ]
        }
    });
    restore_empty_lists(&mut details);
    assert_eq!(details["work"]["work_details"][0]["labors"], json!([]));
    assert_eq!(
        details["work"]["work_details"][1]["labors"],
        json!(["mining"])
    );
}

#[test]
fn empty_work_order_lists_are_restored() {
    let mut result = json!({
        "work_orders": [
            {
                "item_conditions": [{"contains": {}}],
                "order_conditions": {}
            },
            {"item_conditions": [{"comparison": "at_least"}], "order_conditions": []}
        ]
    });
    restore_work_order_lists(&mut result);
    assert_eq!(
        result["work_orders"][0]["item_conditions"][0]["contains"],
        json!([])
    );
    assert_eq!(result["work_orders"][0]["order_conditions"], json!([]));
    assert_eq!(
        result["work_orders"][1]["item_conditions"],
        json!([{"comparison": "at_least"}])
    );
    assert_eq!(result["work_orders"][1]["order_conditions"], json!([]));

    let mut empty = json!({"work_orders": {}});
    restore_work_order_lists(&mut empty);
    assert_eq!(empty["work_orders"], json!([]));
}

#[test]
fn empty_workshop_job_wait_reasons_are_restored() {
    let mut result = json!({
        "workshops": [
            {
                "jobs": [
                    {
                        "wait_reasons": {},
                        "flags": {},
                        "requirements": {}
                    }
                ]
            }
        ]
    });

    restore_workshop_lists(&mut result);

    assert_eq!(result["workshops"][0]["jobs"][0]["wait_reasons"], json!([]));
    assert_eq!(result["workshops"][0]["jobs"][0]["flags"], json!([]));
}

#[test]
fn empty_reaction_definition_lists_are_restored() {
    let mut result = json!({
        "reactions": [
            {
                "allowed_buildings": {},
                "reagents": [{
                    "flags": {},
                    "reagent_flags": {},
                    "contains": {}
                }],
                "products": {},
                "flags": {}
            }
        ]
    });

    restore_reaction_lists(&mut result);

    assert_eq!(result["reactions"][0]["allowed_buildings"], json!([]));
    assert_eq!(result["reactions"][0]["reagents"][0]["flags"], json!([]));
    assert_eq!(
        result["reactions"][0]["reagents"][0]["reagent_flags"],
        json!([])
    );
    assert_eq!(result["reactions"][0]["reagents"][0]["contains"], json!([]));
    assert_eq!(result["reactions"][0]["products"], json!([]));
    assert_eq!(result["reactions"][0]["flags"], json!([]));
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

/// Unwraps the next broadcast event, expecting it to be a new message.
fn try_recv_added(
    receiver: &mut tokio::sync::broadcast::Receiver<crate::notifications::HistoryEvent>,
) -> DisplayMessage {
    match receiver.try_recv().unwrap() {
        crate::notifications::HistoryEvent::Added(_, message) => message,
        other => panic!("expected Added, got {other:?}"),
    }
}

#[test]
fn version_warning_is_notified_only_when_it_changes() {
    let history = NotificationHistory::new();
    let mut notifications_rx = history.subscribe(None).1;
    let notifier = Notifier::new(history);
    let first_warning = Some("first warning".to_owned());
    let mut previous_warning = None;

    notify_version_warning(first_warning.clone(), &mut previous_warning, &notifier);
    assert_eq!(
        try_recv_added(&mut notifications_rx),
        DisplayMessage::new(DisplayMessageKind::Warning, "first warning")
    );

    notify_version_warning(first_warning.clone(), &mut previous_warning, &notifier);
    assert!(notifications_rx.try_recv().is_err());

    let changed_warning = Some("changed warning".to_owned());
    notify_version_warning(changed_warning, &mut previous_warning, &notifier);
    assert_eq!(
        try_recv_added(&mut notifications_rx),
        DisplayMessage::new(DisplayMessageKind::Warning, "changed warning")
    );

    notify_version_warning(None, &mut previous_warning, &notifier);
    notify_version_warning(first_warning, &mut previous_warning, &notifier);
    assert_eq!(
        try_recv_added(&mut notifications_rx),
        DisplayMessage::new(DisplayMessageKind::Warning, "first warning")
    );
}

#[test]
fn lua_pause_result_must_be_a_boolean() {
    assert_eq!(parse_lua_boolean_result("true", "pause"), Ok(true));
    assert_eq!(parse_lua_boolean_result("false", "pause"), Ok(false));
    assert!(parse_lua_boolean_result("", "pause").is_err());
    assert!(parse_lua_boolean_result("true\nerror", "pause").is_err());
}
