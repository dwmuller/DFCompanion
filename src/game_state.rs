use serde_json::{json, Value};
use std::collections::BTreeMap;

const TESTED_DWARF_FORTRESS_VERSION: &str = "53.16";
const TESTED_DFHACK_VERSION: &str = "53.16-r2";
pub const MAX_UNIT_PAGE_SIZE: usize = 50;
const MAX_UNIT_REPLY_BYTES: usize = 12 * 1024;
static LAST_VERSION_WARNING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub async fn list_units(
    offset: usize,
    limit: usize,
    notifications_tx: tokio::sync::broadcast::Sender<String>,
) -> Result<Value, String> {
    if limit == 0 || limit > MAX_UNIT_PAGE_SIZE {
        return Err(format!(
            "The unit list page size must be between 1 and {MAX_UNIT_PAGE_SIZE}."
        ));
    }

    tokio::task::spawn_blocking(move || list_units_blocking(offset, limit, notifications_tx))
        .await
        .map_err(|error| format!("DFHack query task failed: {error}"))?
}

fn list_units_blocking(
    offset: usize,
    limit: usize,
    notifications_tx: tokio::sync::broadcast::Sender<String>,
) -> Result<Value, String> {
    let mut client = dfhack_remote::connect()
        .map_err(|error| format!("Could not connect to the DFHack remote interface: {error}"))?;
    let version = client
        .remote_fortress_reader()
        .get_version_info()
        .map_err(|error| format!("Could not check DFHack/game version: {error}"))?
        .reply;
    let version_warning = version_mismatch_warning(
        version.dwarf_fortress_version.as_deref(),
        version.dfhack_version.as_deref(),
    );
    let mut previous_warning = LAST_VERSION_WARNING
        .lock()
        .map_err(|error| format!("Version warning state is unavailable: {error}"))?;
    notify_version_warning(version_warning, &mut previous_warning, &notifications_tx);
    drop(previous_warning);
    let units = client
        .remote_fortress_reader()
        .get_unit_list()
        .map_err(|error| format!("Could not retrieve the fortress unit list: {error}"))?
        .reply
        .creature_list;

    let total = units.len();
    let mut counts_by_race_id = BTreeMap::<String, usize>::new();
    for unit in &units {
        let race_id = unit
            .race
            .map(|race| race.mat_type.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        *counts_by_race_id.entry(race_id).or_default() += 1;
    }
    let mut page = units
        .iter()
        .skip(offset)
        .take(limit)
        .map(|unit| {
            json!({
                "id": unit.id,
                "name": unit.name.as_deref().filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Unnamed unit {}", unit.id)),
                "race_id": unit.race.map(|race| race.mat_type),
                "caste_id": unit.race.map(|race| race.mat_index),
                "is_soldier": unit.is_soldier,
                "age": unit.age,
                "position": unit.pos_x.zip(unit.pos_y).zip(unit.pos_z)
                    .map(|((x, y), z)| json!({"x": x, "y": y, "z": z})),
            })
        })
        .collect::<Vec<_>>();
    loop {
        let next_offset = offset.saturating_add(page.len());
        let mut result = json!({
            "total_units": total,
            "counts_by_race_id": &counts_by_race_id,
            "offset": offset,
            "returned": page.len(),
            "units": &page,
        });
        if next_offset < total {
            result["next_offset"] = json!(next_offset);
        }
        if result.to_string().len() <= MAX_UNIT_REPLY_BYTES {
            return Ok(result);
        }
        if page.is_empty() {
            return Err("A unit-list response exceeded the 12 KiB size limit.".to_owned());
        }
        page.pop();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
