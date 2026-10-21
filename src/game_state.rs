//! Connection to Dwarf Fortress through DFHack's RemoteFortressReader.
//!
//! [`GameState`] owns one cached DFHack connection. The version check runs only
//! when a connection is (re)established. Query methods validate their arguments,
//! make synchronous DFHack calls, and shape the data for the AI. They block, so
//! async callers should use [`GameState::run`].

use dfhack_remote::Client;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

const TESTED_DWARF_FORTRESS_VERSION: &str = "53.16";
const TESTED_DFHACK_VERSION: &str = "53.16-r2";
pub const MAX_UNIT_PAGE_SIZE: usize = 50;
const MAX_UNIT_REPLY_BYTES: usize = 12 * 1024;

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

    /// Returns a page of fortress units with a per-race summary.
    pub fn list_units(&self, offset: usize, limit: usize) -> Result<Value, String> {
        if !(1..=MAX_UNIT_PAGE_SIZE).contains(&limit) {
            return Err(format!(
                "The unit list page size must be between 1 and {MAX_UNIT_PAGE_SIZE}."
            ));
        }
        let units = self
            .with_client("Could not retrieve the fortress unit list", |client| {
                client.remote_fortress_reader().get_unit_list()
            })?
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
