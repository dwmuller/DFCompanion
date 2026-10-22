//! Interactive tests that talk to a running Dwarf Fortress with DFHack.
//!
//! These are all `#[ignore]`d so `cargo test` skips them. Run them with:
//!
//! ```text
//! cargo test --test live_game_tests -- --ignored --nocapture --test-threads=1
//! cargo test --test live_game_tests -- --ignored --nocapture creature_raws
//! ```
//!
//! The game must be running with DFHack's remote server reachable on
//! `127.0.0.1:5000` (override with the `DFHACK_PORT` environment variable).
//!
//! The typed tests use `dfhack-remote`, which decodes replies with prost; one
//! string that is not valid UTF-8 (DFHack sometimes sends CP437) fails the
//! whole reply. The `raw_*` tests bypass prost, walk the protobuf wire format
//! by hand and report every string field that is not valid UTF-8, with its
//! field-number path (for example `1.8.20`).

use dfhack_remote::{BlockRequest, EmptyMessage, ListRequest};
use prost_014::{Message as _, Name as _};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

const PAGE: i32 = 10;
const MAX_CREATURE_RAWS: i32 = 5000;

fn connect() -> dfhack_remote::Client {
    dfhack_remote::connect().unwrap_or_else(|error| {
        panic!("Could not connect to DFHack (is the game running with remote enabled?): {error}")
    })
}

fn show<T>(label: &str, result: &Result<dfhack_remote::Reply<T>, dfhack_remote::Error>) -> bool {
    match result {
        Ok(_) => {
            println!("[ok]   {label}");
            true
        }
        Err(error) => {
            println!("[FAIL] {label}: {error}");
            false
        }
    }
}

#[test]
#[ignore = "requires a running game"]
fn version_info() {
    let mut client = connect();
    let version = client
        .remote_fortress_reader()
        .get_version_info()
        .expect("GetVersionInfo failed")
        .reply;
    println!(
        "DF {:?}, DFHack {:?}",
        version.dwarf_fortress_version, version.dfhack_version
    );
}

#[test]
#[ignore = "requires a running game"]
fn creature_raws() {
    let mut client = connect();
    let result = client.remote_fortress_reader().get_creature_raws();
    if show("GetCreatureRaws", &result) {
        println!("       {} creature raws", result.unwrap().reply.creature_raws.len());
    } else {
        panic!("GetCreatureRaws failed");
    }
}

/// Pages through GetPartialCreatureRaws and bisects failing pages down to the
/// individual creature indices that cannot be decoded.
#[test]
#[ignore = "requires a running game"]
fn creature_raws_paged() {
    let mut client = connect();
    let mut bad = Vec::new();
    let mut start = 0;
    while start < MAX_CREATURE_RAWS {
        let end = start + PAGE;
        match partial_raws(&mut client, start, end) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                println!("[FAIL] page {start}..{end}: {error}; bisecting");
                bisect(&mut client, start, end, &mut bad);
            }
        }
        start = end;
    }
    for (index, error) in &bad {
        println!("creature raw index {index} fails to decode: {error}");
    }
    assert!(bad.is_empty(), "{} creature raws failed to decode", bad.len());
}

fn partial_raws(
    client: &mut dfhack_remote::Client,
    start: i32,
    end: i32,
) -> Result<usize, dfhack_remote::Error> {
    client
        .remote_fortress_reader()
        .get_partial_creature_raws(ListRequest {
            list_start: Some(start),
            list_end: Some(end),
        })
        .map(|reply| reply.reply.creature_raws.len())
}

fn bisect(
    client: &mut dfhack_remote::Client,
    start: i32,
    end: i32,
    bad: &mut Vec<(i32, String)>,
) {
    let Err(error) = partial_raws(client, start, end) else {
        return;
    };
    if end - start <= 1 {
        bad.push((start, error.to_string()));
        return;
    }
    let mid = start + (end - start) / 2;
    bisect(client, start, mid, bad);
    bisect(client, mid, end, bad);
}

#[test]
#[ignore = "requires a running game"]
fn unit_lists() {
    let mut client = connect();
    let all = client.remote_fortress_reader().get_unit_list();
    let all_ok = show("GetUnitList", &all);
    if let Ok(reply) = &all {
        println!("       {} units", reply.reply.creature_list.len());
    }
    let inside = client
        .remote_fortress_reader()
        .get_unit_list_inside(whole_map_request());
    let inside_ok = show("GetUnitListInside", &inside);
    if let Ok(reply) = &inside {
        println!("       {} units", reply.reply.creature_list.len());
    }
    assert!(all_ok && inside_ok, "a unit list call failed");
}

#[test]
#[ignore = "requires a running game"]
fn material_and_item_lists() {
    let mut client = connect();
    let mut ok = true;
    ok &= show("GetMaterialList", &client.remote_fortress_reader().get_material_list());
    ok &= show("GetGrowthList", &client.remote_fortress_reader().get_growth_list());
    ok &= show("GetItemList", &client.remote_fortress_reader().get_item_list());
    ok &= show("GetPlantRaws", &client.remote_fortress_reader().get_plant_raws());
    assert!(ok, "a list call failed");
}

/// Drop any cached copy of the Lua module from an earlier version.
fn reload_lua_module() {
    let mut client = connect();
    let _ = client.core().run_command(dfhack_remote::CoreRunCommandRequest {
        command: "lua".to_owned(),
        arguments: vec!["package.loaded['rpc.dfcompanion']=nil".to_owned()],
    });
}

/// Every AI tool, through the same dispatcher the model uses.
#[tokio::test]
#[ignore = "requires a running game"]
async fn ai_tools_end_to_end() {
    use dfcompanion::ai_tools::execute_named;
    reload_lua_module();
    let game_state = dfcompanion::game_state::GameState::new(tokio::sync::broadcast::channel(16).0);
    let call = |name: &'static str, args: serde_json::Value| {
        let game_state = game_state.clone();
        async move {
            let text = execute_named(&game_state, name, args).await;
            println!("{name}: {text}");
            serde_json::from_str::<serde_json::Value>(&text).unwrap()
        }
    };
    let overview = call("get_fortress_overview", serde_json::json!({})).await;
    assert!(overview.get("error").is_none(), "{overview}");
    let mut first_id = None;
    for tool in ["list_residents", "list_pets_livestock", "list_other_creatures", "list_dead_missing"] {
        let list = call(tool, serde_json::json!({"limit": 5})).await;
        assert!(list.get("error").is_none(), "{list}");
        if first_id.is_none() {
            first_id = list["creatures"][0]["id"].as_i64();
        }
    }
    if let Some(id) = first_id {
        let details = call("get_unit_details", serde_json::json!({"unit_id": id})).await;
        assert!(details.get("error").is_none(), "{details}");
        // The first id comes from the residents list when there are residents.
        let mind = call("get_unit_mind", serde_json::json!({"unit_id": id})).await;
        assert!(mind.get("error").is_none(), "{mind}");
        assert!(mind["mood"].is_string() && mind["recent_thoughts"].is_array(), "{mind}");
        assert!(mind["unmet_needs"].is_array(), "{mind}");
    }
    let no_mind = call("get_unit_mind", serde_json::json!({"unit_id": 999_999_999})).await;
    assert!(no_mind.get("error").is_some(), "{no_mind}");
    let unknown = call("get_unit_details", serde_json::json!({"unit_id": 999_999_999})).await;
    assert!(unknown.get("error").is_some(), "{unknown}");
    let reports = call("get_recent_reports", serde_json::json!({"limit": 5})).await;
    assert!(reports.get("error").is_none(), "{reports}");
    assert!(!reports["reports"].as_array().unwrap().is_empty(), "{reports}");
    let date = overview["game_date"].as_str().expect("game_date string");
    let today = &date[..date.rfind(':').unwrap()];
    let ranged = call("get_recent_reports", serde_json::json!({"from": "000-01-01", "to": today})).await;
    assert!(ranged.get("error").is_none(), "{ranged}");
    let none = call("get_recent_reports", serde_json::json!({"from": "9999-01-01"})).await;
    assert_eq!(none["total_matching"], 0, "{none}");
    let bad = call("get_recent_reports", serde_json::json!({"from": "nope"})).await;
    assert!(bad.get("error").is_some(), "{bad}");
    let deaths = call("get_recent_reports", serde_json::json!({"category": "death", "limit": 50})).await;
    assert!(deaths.get("error").is_none(), "{deaths}");
    assert!(deaths["reports"].as_array().unwrap().iter().all(|r| r["category"] == "death"), "{deaths}");
    let bad = call("get_recent_reports", serde_json::json!({"category": "nope"})).await;
    assert!(bad.get("error").is_some(), "{bad}");
    // Paging forward by id: the oldest reports after an id, with no gaps.
    let latest_id = reports["latest_id"].as_i64().expect("latest_id");
    let newest_id = reports["reports"].as_array().unwrap().last().unwrap()["id"].as_i64().unwrap();
    assert!(latest_id >= newest_id, "{reports}");
    let nothing_newer = call("get_recent_reports", serde_json::json!({"since_id": latest_id})).await;
    assert_eq!(nothing_newer["total_matching"], 0, "{nothing_newer}");
    let page = call("get_recent_reports", serde_json::json!({"since_id": newest_id - 200, "limit": 3})).await;
    assert!(page.get("error").is_none(), "{page}");
    let ids: Vec<i64> = page["reports"].as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert!(!ids.is_empty() && ids.iter().all(|id| *id > newest_id - 200) && ids.windows(2).all(|w| w[0] < w[1]), "{page}");
    let bounded = call("get_recent_reports", serde_json::json!({"since_id": newest_id - 200, "until_id": ids[0], "limit": 50})).await;
    let bounded_ids: Vec<i64> = bounded["reports"].as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(bounded_ids, vec![ids[0]], "{bounded}");
    let items = call("search_items", serde_json::json!({"item_type": "WOOD", "limit": 5})).await;
    assert!(items.get("error").is_none(), "{items}");
    assert!(items["total_matching"].as_u64().unwrap() > 0, "{items}");
    let bad = call("search_items", serde_json::json!({"item_type": "NOPE"})).await;
    assert!(bad.get("error").is_some(), "{bad}");
    let stocks = call("get_food_drink_summary", serde_json::json!({})).await;
    assert!(stocks.get("error").is_none(), "{stocks}");
    assert!(stocks["population"].as_u64().unwrap() > 0, "{stocks}");
    let drinks = call("search_items", serde_json::json!({"item_type": "DRINK", "limit": 1})).await;
    assert_eq!(
        stocks["drink"]["total"].as_u64(),
        drinks["quantity_by_type"]["drink"].as_u64().or(Some(0)),
        "{stocks} {drinks}"
    );
    assert!(stocks["food"]["available"].as_u64().unwrap() <= stocks["food"]["total"].as_u64().unwrap(), "{stocks}");
}

fn whole_map_request() -> BlockRequest {
    BlockRequest {
        min_x: Some(0),
        max_x: Some(10_000),
        min_y: Some(0),
        max_y: Some(10_000),
        min_z: Some(0),
        max_z: Some(1_000),
        ..Default::default()
    }
}

#[test]
#[ignore = "requires a running game"]
fn raw_creature_raws_utf8() {
    let found = scan(
        "GetCreatureRaws",
        dfhack_remote::CreatureRawList::full_name(),
        Vec::new(),
    );
    assert_eq!(found, 0, "non-UTF-8 strings found");
}

#[test]
#[ignore = "requires a running game"]
fn raw_unit_list_utf8() {
    let found = scan("GetUnitList", dfhack_remote::UnitList::full_name(), Vec::new());
    assert_eq!(found, 0, "non-UTF-8 strings found");
}

#[test]
#[ignore = "requires a running game"]
fn raw_unit_list_inside_utf8() {
    let found = scan(
        "GetUnitListInside",
        dfhack_remote::UnitList::full_name(),
        whole_map_request().encode_to_vec(),
    );
    assert_eq!(found, 0, "non-UTF-8 strings found");
}

#[test]
#[ignore = "requires a running game"]
fn raw_material_and_item_lists_utf8() {
    let material = dfhack_remote::MaterialList::full_name();
    let mut found = 0;
    for method in ["GetMaterialList", "GetGrowthList", "GetItemList"] {
        found += scan(method, material.clone(), Vec::new());
    }
    found += scan(
        "GetPlantRaws",
        dfhack_remote::PlantRawList::full_name(),
        Vec::new(),
    );
    assert_eq!(found, 0, "non-UTF-8 strings found");
}

/// Calls a RemoteFortressReader method without decoding the reply and prints
/// every string-like field that is not valid UTF-8. Returns how many were found.
fn scan(method: &str, output_msg: String, request: Vec<u8>) -> usize {
    let input_msg = if request.is_empty() {
        EmptyMessage::full_name()
    } else {
        BlockRequest::full_name()
    };
    let mut raw = RawClient::connect();
    let reply = raw
        .call("RemoteFortressReader", method, &input_msg, &output_msg, &request)
        .unwrap_or_else(|error| panic!("{method} failed: {error}"));
    let mut findings = Vec::new();
    let well_formed = scan_message(&reply, "", &mut findings);
    println!(
        "{method}: {} bytes, well-formed protobuf: {well_formed}, {} non-UTF-8 string(s)",
        reply.len(),
        findings.len()
    );
    let total = findings.len();
    let mut by_path = BTreeMap::<String, (usize, Vec<u8>)>::new();
    for (path, bytes) in findings {
        by_path.entry(generalize(&path)).or_insert((0, bytes)).0 += 1;
    }
    for (path, (count, example)) in &by_path {
        println!(
            "  field {path}: {count} occurrence(s); example bytes \"{}\" => lossy \"{}\"",
            example.escape_ascii(),
            String::from_utf8_lossy(example)
        );
    }
    total
}

/// "1[12].8[3].20" -> "1.8.20", so occurrences are grouped by field.
fn generalize(path: &str) -> String {
    let mut out = String::new();
    let mut in_index = false;
    for c in path.chars() {
        match c {
            '[' => in_index = true,
            ']' => in_index = false,
            _ if !in_index => out.push(c),
            _ => {}
        }
    }
    out
}

struct RawClient {
    stream: TcpStream,
    bound: BTreeMap<String, i16>,
}

impl RawClient {
    fn connect() -> Self {
        let port = std::env::var("DFHACK_PORT").unwrap_or_else(|_| "5000".to_owned());
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .unwrap_or_else(|error| panic!("Could not connect to DFHack: {error}"));
        stream.write_all(b"DFHack?\n").unwrap();
        stream.write_all(&1i32.to_le_bytes()).unwrap();
        let mut reply = [0u8; 12];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply[..8], b"DFHack!\n", "unexpected handshake reply");
        Self {
            stream,
            bound: BTreeMap::new(),
        }
    }

    fn call(
        &mut self,
        plugin: &str,
        method: &str,
        input_msg: &str,
        output_msg: &str,
        request: &[u8],
    ) -> Result<Vec<u8>, String> {
        let key = format!("{plugin}:{method}");
        let id = match self.bound.get(&key) {
            Some(id) => *id,
            None => {
                let bind = dfhack_remote::CoreBindRequest {
                    method: method.to_owned(),
                    input_msg: input_msg.to_owned(),
                    output_msg: output_msg.to_owned(),
                    plugin: Some(plugin.to_owned()),
                };
                let reply = self
                    .round_trip(0, &bind.encode_to_vec())
                    .map_err(|error| format!("could not bind {key}: {error}"))?;
                let reply = dfhack_remote::CoreBindReply::decode(reply.as_slice())
                    .map_err(|error| error.to_string())?;
                let id = reply.assigned_id as i16;
                self.bound.insert(key, id);
                id
            }
        };
        self.round_trip(id, request)
    }

    fn round_trip(&mut self, id: i16, payload: &[u8]) -> Result<Vec<u8>, String> {
        let io = |error: std::io::Error| error.to_string();
        let mut header = Vec::with_capacity(8);
        header.extend_from_slice(&id.to_le_bytes());
        header.extend_from_slice(&0i16.to_le_bytes());
        header.extend_from_slice(&(payload.len() as i32).to_le_bytes());
        self.stream.write_all(&header).map_err(io)?;
        self.stream.write_all(payload).map_err(io)?;
        loop {
            let mut header = [0u8; 8];
            self.stream.read_exact(&mut header).map_err(io)?;
            let code = i16::from_le_bytes([header[0], header[1]]);
            let size = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            if code == -2 {
                return Err(format!("RPC failure, command result {size}"));
            }
            let mut body = vec![0u8; size.max(0) as usize];
            self.stream.read_exact(&mut body).map_err(io)?;
            match code {
                -1 => return Ok(body),
                -3 => continue,
                other => return Err(format!("unexpected reply code {other}")),
            }
        }
    }
}

impl Drop for RawClient {
    fn drop(&mut self) {
        let mut header = Vec::with_capacity(8);
        header.extend_from_slice(&(-4i16).to_le_bytes());
        header.extend_from_slice(&0i16.to_le_bytes());
        header.extend_from_slice(&0i32.to_le_bytes());
        let _ = self.stream.write_all(&header);
    }
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f).checked_shl(shift).unwrap_or(0);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Strictly splits a buffer into (field number, length-delimited payload) pairs.
/// Returns None if it is not a well-formed protobuf message.
fn parse_fields(buf: &[u8]) -> Option<Vec<(u64, Option<&[u8]>)>> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let number = tag >> 3;
        if number == 0 {
            return None;
        }
        match tag & 7 {
            0 => {
                read_varint(buf, &mut pos)?;
                fields.push((number, None));
            }
            1 => {
                pos = pos.checked_add(8).filter(|&end| end <= buf.len())?;
                fields.push((number, None));
            }
            5 => {
                pos = pos.checked_add(4).filter(|&end| end <= buf.len())?;
                fields.push((number, None));
            }
            2 => {
                let len = usize::try_from(read_varint(buf, &mut pos)?).ok()?;
                let end = pos.checked_add(len).filter(|&end| end <= buf.len())?;
                fields.push((number, Some(&buf[pos..end])));
                pos = end;
            }
            _ => return None,
        }
    }
    Some(fields)
}

/// Heuristically decides, per length-delimited field, whether it is a nested
/// message or a string. Text that is valid UTF-8 without control characters is
/// a string; anything else that parses as a message is recursed into; the rest
/// is a string, and reported if it is not valid UTF-8.
fn scan_message(buf: &[u8], path: &str, findings: &mut Vec<(String, Vec<u8>)>) -> bool {
    let Some(fields) = parse_fields(buf) else {
        return false;
    };
    let mut counts = BTreeMap::<u64, usize>::new();
    for (number, payload) in fields {
        let index = counts.entry(number).or_default();
        let field_path = if path.is_empty() {
            format!("{number}[{index}]")
        } else {
            format!("{path}.{number}[{index}]")
        };
        *index += 1;
        let Some(payload) = payload else { continue };
        let clean_text = std::str::from_utf8(payload)
            .map(|text| !text.chars().any(char::is_control))
            .unwrap_or(false);
        if clean_text || payload.is_empty() {
            continue;
        }
        if !scan_message(payload, &field_path, findings) && std::str::from_utf8(payload).is_err() {
            findings.push((field_path, payload.to_vec()));
        }
    }
    true
}

#[test]
fn wire_scanner_finds_invalid_utf8_strings() {
    // field 1 = message { field 2 = "ok", field 3 = [0x82, 0x83] }
    let inner = [&[0x12, 2][..], b"ok", &[0x1a, 2, 0x82, 0x83]].concat();
    let outer = [&[0x0a, inner.len() as u8][..], &inner].concat();
    let mut findings = Vec::new();
    assert!(scan_message(&outer, "", &mut findings));
    assert_eq!(findings, vec![("1[0].3[0]".to_owned(), vec![0x82, 0x83])]);
    assert_eq!(generalize("1[12].8[3].20"), "1.8.20");
}
