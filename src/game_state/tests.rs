use crate::notifications::NotificationHistory;

use super::*;

#[test]
fn should_round_trip_chat_state_when_compressed_for_rpc() {
    let state = "{\"message\":\"Dwarves: \u{00e5}, \u{2603}, \\u0000\"}".repeat(64);
    let encoded = encode_chat_state(&state).unwrap();
    assert!(encoded.len() < state.len());
    assert_eq!(
        decode_chat_state(&format!("zstd:{encoded}")).unwrap(),
        Some(state)
    );
}

#[test]
fn should_restore_legacy_chat_state_without_compression() {
    let state = r#"{"version":1,"messages":[]}"#;
    assert_eq!(
        decode_chat_state(&format!("json:{state}")).unwrap(),
        Some(state.to_owned())
    );
}

#[test]
fn should_report_no_chat_state_when_reply_is_empty() {
    assert_eq!(decode_chat_state("").unwrap(), None);
}

#[test]
fn should_reject_corrupt_or_unrecognized_chat_state_encoding() {
    for reply in ["unknown:data", "zstd:!!!!", "zstd:YWJj"] {
        assert!(decode_chat_state(reply).is_err(), "{reply}");
    }
}

#[test]
fn should_reject_chat_state_when_decompressed_data_is_not_utf8() {
    let encoded = BASE64.encode(zstd::stream::encode_all(&[0xff][..], 0).unwrap());
    assert!(decode_chat_state(&format!("zstd:{encoded}"))
        .unwrap_err()
        .contains("not UTF-8"));
}

#[test]
fn should_reject_chat_state_when_decompressed_size_exceeds_limit() {
    let state = "a".repeat(crate::ai::MAX_SNAPSHOT_BYTES + 1);
    let encoded = BASE64.encode(zstd::stream::encode_all(state.as_bytes(), 0).unwrap());
    assert!(decode_chat_state(&format!("zstd:{encoded}"))
        .unwrap_err()
        .contains("size limit"));
}

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
        "site_id": 41,
        "session_id": "a"
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
fn should_reject_loaded_lifecycle_when_session_is_missing() {
    let snapshot = LifecycleSnapshot {
        world_loaded: true,
        map_loaded: true,
        fortress_name: "Bridgedmine".to_owned(),
        site_id: Some(41),
        session_id: None,
    };
    assert!(snapshot.validate().is_err());
}

#[test]
fn empty_detail_lists_are_restored() {
    let mut details = json!({
        "skills": {},
        "mind": {"recent_thoughts": {}, "unmet_needs": [1]},
        "inventory": {"items": {}},
        "relations": {"affiliations": {}, "worshipped_deities": {}},
        "work": {"work_details": {}}
    });
    restore_empty_lists(&mut details);
    assert_eq!(details["skills"], json!([]));
    assert_eq!(details["mind"]["recent_thoughts"], json!([]));
    assert_eq!(details["mind"]["unmet_needs"], json!([1]));
    assert_eq!(details["inventory"]["items"], json!([]));
    assert_eq!(details["relations"]["affiliations"], json!([]));
    assert_eq!(details["relations"]["worshipped_deities"], json!([]));
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
fn relations_is_a_supported_detail_part_and_unknown_parts_are_rejected() {
    assert!(validate_detail_parts(&["relations".to_owned()]).is_ok());
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
fn should_preserve_nested_workshop_links_without_adding_top_level_fields() {
    let mut result = json!({
        "workshops": [{
            "jobs": {},
            "stockpile_links": {
                "give_to_stockpiles": {},
                "take_from_stockpiles": {},
                "give_to_workshops": {},
                "take_from_workshops": {}
            }
        }, {
            "jobs": [],
            "stockpile_links": {
                "give_to_stockpiles": [{"id": 1}],
                "take_from_stockpiles": [{"id": 2}],
                "give_to_workshops": [{"id": 3}],
                "take_from_workshops": [{"id": 4}]
            }
        }]
    });

    restore_workshop_lists(&mut result);

    assert_eq!(
        result,
        json!({
            "workshops": [{
                "jobs": [],
                "stockpile_links": {
                    "give_to_stockpiles": [],
                    "take_from_stockpiles": [],
                    "give_to_workshops": [],
                    "take_from_workshops": []
                }
            }, {
                "jobs": [],
                "stockpile_links": {
                    "give_to_stockpiles": [{"id": 1}],
                    "take_from_stockpiles": [{"id": 2}],
                    "give_to_workshops": [{"id": 3}],
                    "take_from_workshops": [{"id": 4}]
                }
            }]
        })
    );
}

#[test]
fn should_join_matching_job_requirements_to_their_manager_order() {
    let workshops = vec![json!({
        "id": 10,
        "name": "Smelter",
        "jobs": [
            {
                "id": 4,
                "order_id": 7,
                "reaction": "MAKE_IRON_BARS",
                "requirements": [{"material": "IRON"}],
                "wait_reasons": ["waiting_for_required_items"]
            },
            {"id": 5, "order_id": 8, "reaction": "OTHER"}
        ]
    })];
    let work_orders = vec![
        json!({"id": 7, "name": "Make bars", "order_conditions": []}),
        json!({"id": 8, "name": "Other order", "order_conditions": []}),
    ];

    let (selected_workshops, selected_orders) =
        filter_production_evidence(&workshops, &work_orders, Some("iron"), None, None);

    assert_eq!(selected_workshops[0]["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(selected_workshops[0]["jobs"][0]["id"], 4);
    assert_eq!(selected_orders.len(), 1);
    assert_eq!(selected_orders[0]["id"], 7);
    let signals = production_block_signals(&selected_workshops, &selected_orders);
    assert_eq!(signals[0]["signal"], "wait_reason");
    assert_eq!(signals[0]["value"], "waiting_for_required_items");
}

#[test]
fn should_scope_manager_orders_to_selected_workshop_and_surface_order_conditions() {
    let workshops = vec![json!({
        "id": 10,
        "name": "Smelter",
        "jobs": [{"id": 4, "order_id": 7, "wait_reasons": [], "remaining_dimension": 0}]
    })];
    let work_orders = vec![
        json!({
            "id": 7,
            "workshop_id": 10,
            "is_active": true,
            "is_validated": true,
            "order_conditions": [{"condition": "completed", "satisfied": false}]
        }),
        json!({"id": 8, "workshop_id": 11, "order_conditions": []}),
    ];

    let (selected_workshops, selected_orders) =
        filter_production_evidence(&workshops, &work_orders, None, Some(10), None);

    assert_eq!(selected_workshops.len(), 1);
    assert_eq!(selected_orders.len(), 1);
    assert_eq!(selected_orders[0]["id"], 7);
    let signals = production_block_signals(&selected_workshops, &selected_orders);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["signal"], "unsatisfied_order_condition");
    assert_eq!(signals[0]["condition"], "completed");
}

#[test]
fn should_restore_nested_empty_stockpile_link_lists() {
    let mut result = json!({
        "stockpiles": [{
            "stockpile_links": {
                "give_to_stockpiles": {},
                "take_from_stockpiles": [],
                "give_to_workshops": {},
                "take_from_workshops": []
            }
        }]
    });

    restore_stockpile_lists(&mut result);

    assert_eq!(
        result["stockpiles"][0]["stockpile_links"]["give_to_stockpiles"],
        json!([])
    );
    assert_eq!(
        result["stockpiles"][0]["stockpile_links"]["give_to_workshops"],
        json!([])
    );
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
