use std::collections::BTreeMap;

use coordination_core::{CoordinationError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::types::find_string;
use crate::{
    AdapterReadiness, AdapterStatusSnapshot, CcpanesAdapter, SanitizedEndpointConfig,
    CCPANES_ADAPTER_CONTRACT_VERSION,
};

pub const READ_ONLY_PREFLIGHT_CONTRACT_VERSION: &str = "ccpanes-adapter-readonly-preflight/v1beta1";

const READ_ONLY_OPERATIONS: &[&str] = &[
    "initialize",
    "notifications/initialized",
    "tools/list",
    "list_sessions",
    "list_panes",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadOnlyPreflightReport {
    pub contract_version: String,
    pub adapter_contract_version: String,
    pub readiness: AdapterReadiness,
    pub endpoint: SanitizedEndpointConfig,
    pub read_only_operations: Vec<String>,
    pub external_worker_side_effects: bool,
    pub status: AdapterStatusSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_inventory: Option<SessionInventorySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CoordinationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionInventorySnapshot {
    pub session_count: u64,
    pub active_session_count: u64,
    pub terminal_session_count: u64,
    pub pane_count: u64,
    pub tab_count: u64,
    pub status_counts: BTreeMap<String, u64>,
    pub session_id_digests: Vec<String>,
}

pub fn run_read_only_preflight(adapter: &mut CcpanesAdapter) -> ReadOnlyPreflightReport {
    let endpoint = adapter.config().sanitized();
    match adapter.discover() {
        Ok(capability) => match read_session_inventory(adapter) {
            Ok(session_inventory) => ReadOnlyPreflightReport {
                contract_version: READ_ONLY_PREFLIGHT_CONTRACT_VERSION.to_owned(),
                adapter_contract_version: CCPANES_ADAPTER_CONTRACT_VERSION.to_owned(),
                readiness: AdapterReadiness::Ready,
                endpoint,
                read_only_operations: read_only_operations(),
                external_worker_side_effects: false,
                status: adapter.status_snapshot(),
                session_inventory: Some(session_inventory),
                error: None,
            },
            Err(error) => {
                degraded_report(endpoint, adapter.status_snapshot(), Some(capability), error)
            }
        },
        Err(error) => {
            let readiness = readiness_for_error(&error);
            ReadOnlyPreflightReport {
                contract_version: READ_ONLY_PREFLIGHT_CONTRACT_VERSION.to_owned(),
                adapter_contract_version: CCPANES_ADAPTER_CONTRACT_VERSION.to_owned(),
                readiness,
                endpoint,
                read_only_operations: read_only_operations(),
                external_worker_side_effects: false,
                status: adapter.status_snapshot(),
                session_inventory: None,
                error: Some(error),
            }
        }
    }
}

pub fn require_dispatch_requalification(
    report: &ReadOnlyPreflightReport,
) -> Result<(), CoordinationError> {
    if let Some(error) = &report.error {
        return Err(error.clone());
    }
    match report.readiness {
        AdapterReadiness::Ready => {}
        AdapterReadiness::Degraded => {
            return Err(CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "CC-Panes read-only preflight is degraded",
            ));
        }
        AdapterReadiness::Incompatible => {
            return Err(CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                "CC-Panes read-only preflight is incompatible",
            ));
        }
    }
    if report.external_worker_side_effects {
        return Err(CoordinationError::new(
            ErrorCode::InternalInvariantViolation,
            "CC-Panes read-only preflight reported external Worker side effects",
        ));
    }
    let capability = report.status.capability.as_ref().ok_or_else(|| {
        CoordinationError::new(
            ErrorCode::AdapterNotReady,
            "CC-Panes read-only preflight did not produce a capability snapshot",
        )
    })?;
    if !capability.dispatch_ready {
        return Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            "CC-Panes dispatch requalification requires get_session_status and get_session_output",
        ));
    }
    if report.session_inventory.is_none() {
        return Err(CoordinationError::new(
            ErrorCode::AdapterNotReady,
            "CC-Panes read-only preflight did not produce a Session inventory",
        ));
    }
    Ok(())
}

pub fn session_inventory_from_values(
    sessions: &Value,
    panes: &Value,
) -> Result<SessionInventorySnapshot, CoordinationError> {
    let mut session_count = 0_u64;
    let mut active_session_count = 0_u64;
    let mut terminal_session_count = 0_u64;
    let mut status_counts = BTreeMap::new();
    let mut session_id_digests = Vec::new();

    for session in session_entries(sessions)? {
        session_count = session_count.saturating_add(1);
        let status = session_status(session);
        *status_counts.entry(status.clone()).or_insert(0) += 1;
        if is_terminal_status(&status) {
            terminal_session_count = terminal_session_count.saturating_add(1);
        } else {
            active_session_count = active_session_count.saturating_add(1);
        }
        if let Some(session_id) = find_string(session, &["sessionId", "session_id", "id"]) {
            session_id_digests.push(digest_identifier(&session_id));
        }
    }
    session_id_digests.sort();
    session_id_digests.dedup();

    let pane_entries = pane_entries(panes)?;
    let pane_count = pane_entries.len() as u64;
    let mut tab_count = 0_u64;
    for pane in pane_entries {
        let Some(tabs) = pane.get("tabs") else {
            continue;
        };
        let tabs = tabs
            .as_array()
            .ok_or_else(|| incompatible_inventory_shape("list_panes", "panes[].tabs array"))?;
        if tabs.iter().any(|tab| !tab.is_object()) {
            return Err(incompatible_inventory_shape(
                "list_panes",
                "panes[].tabs[] object entries",
            ));
        }
        tab_count = tab_count.saturating_add(tabs.len() as u64);
    }

    Ok(SessionInventorySnapshot {
        session_count,
        active_session_count,
        terminal_session_count,
        pane_count,
        tab_count,
        status_counts,
        session_id_digests,
    })
}

fn read_session_inventory(
    adapter: &mut CcpanesAdapter,
) -> Result<SessionInventorySnapshot, CoordinationError> {
    let sessions = adapter.list_sessions()?;
    let panes = adapter.list_panes()?;
    session_inventory_from_values(&sessions, &panes)
}

fn degraded_report(
    endpoint: SanitizedEndpointConfig,
    mut status: AdapterStatusSnapshot,
    capability: Option<crate::CapabilitySnapshot>,
    error: CoordinationError,
) -> ReadOnlyPreflightReport {
    status.readiness = readiness_for_error(&error);
    status.capability = capability.or(status.capability);
    ReadOnlyPreflightReport {
        contract_version: READ_ONLY_PREFLIGHT_CONTRACT_VERSION.to_owned(),
        adapter_contract_version: CCPANES_ADAPTER_CONTRACT_VERSION.to_owned(),
        readiness: status.readiness,
        endpoint,
        read_only_operations: read_only_operations(),
        external_worker_side_effects: false,
        status,
        session_inventory: None,
        error: Some(error),
    }
}

fn readiness_for_error(error: &CoordinationError) -> AdapterReadiness {
    if error.code == ErrorCode::AdapterIncompatible {
        AdapterReadiness::Incompatible
    } else {
        AdapterReadiness::Degraded
    }
}

fn read_only_operations() -> Vec<String> {
    READ_ONLY_OPERATIONS
        .iter()
        .map(|operation| (*operation).to_owned())
        .collect()
}

fn session_entries(value: &Value) -> Result<Vec<&Value>, CoordinationError> {
    if let Some(sessions) = value.get("sessions") {
        let sessions = sessions
            .as_array()
            .ok_or_else(|| incompatible_inventory_shape("list_sessions", "sessions"));
        return object_entries("list_sessions", "sessions[] object entries", sessions?);
    }
    if let Some(items) = value.get("items") {
        let items = items
            .as_array()
            .ok_or_else(|| incompatible_inventory_shape("list_sessions", "items"));
        return object_entries("list_sessions", "items[] object entries", items?);
    }
    if let Some(sessions) = value.as_array() {
        return object_entries("list_sessions", "array object entries", sessions);
    }
    Err(incompatible_inventory_shape(
        "list_sessions",
        "sessions/items/array",
    ))
}

fn pane_entries(value: &Value) -> Result<Vec<&Value>, CoordinationError> {
    if let Some(panes) = value.get("panes") {
        let panes = panes
            .as_array()
            .ok_or_else(|| incompatible_inventory_shape("list_panes", "panes"));
        return object_entries("list_panes", "panes[] object entries", panes?);
    }
    if let Some(layouts) = value.get("layouts") {
        let layouts = layouts
            .as_array()
            .ok_or_else(|| incompatible_inventory_shape("list_panes", "layouts"))?;
        let mut panes = Vec::new();
        for layout in layouts {
            if !layout.is_object() {
                return Err(incompatible_inventory_shape(
                    "list_panes",
                    "layouts[] object entries",
                ));
            }
            let layout_panes = layout
                .get("panes")
                .and_then(Value::as_array)
                .ok_or_else(|| incompatible_inventory_shape("list_panes", "layouts[].panes"))?;
            if layout_panes.iter().any(|pane| !pane.is_object()) {
                return Err(incompatible_inventory_shape(
                    "list_panes",
                    "layouts[].panes[] object entries",
                ));
            }
            panes.extend(layout_panes);
        }
        return Ok(panes);
    }
    Err(incompatible_inventory_shape("list_panes", "panes/layouts"))
}

fn object_entries<'a>(
    tool_name: &str,
    expected: &str,
    entries: &'a [Value],
) -> Result<Vec<&'a Value>, CoordinationError> {
    if entries.iter().any(|entry| !entry.is_object()) {
        return Err(incompatible_inventory_shape(tool_name, expected));
    }
    Ok(entries.iter().collect())
}

fn incompatible_inventory_shape(tool_name: &str, expected: &str) -> CoordinationError {
    CoordinationError::new(
        ErrorCode::AdapterIncompatible,
        format!("CC-Panes {tool_name} response must contain {expected}"),
    )
}

fn session_status(value: &Value) -> String {
    let raw = find_string(value, &["status", "state"]).unwrap_or_else(|| "unknown".to_owned());
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "unknown".to_owned()
    } else {
        normalized.chars().take(64).collect()
    }
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "succeeded"
            | "success"
            | "failed"
            | "error"
            | "killed"
            | "cancelled"
            | "canceled"
            | "closed"
            | "exited"
    )
}

fn digest_identifier(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn session_inventory_redacts_session_identity_and_prompt_surface() {
        let inventory = session_inventory_from_values(
            &json!({
                "sessions": [
                    {"sessionId": "session-secret-1", "status": "running"},
                    {"id": "session-secret-2", "state": "completed"}
                ]
            }),
            &json!({
                "panes": [
                    {"tabs": [
                        {
                            "sessionId": "session-secret-1",
                            "title": "SECRET_TITLE",
                            "prompt": "SECRET_PROMPT"
                        }
                    ]}
                ]
            }),
        )
        .unwrap();

        assert_eq!(inventory.session_count, 2);
        assert_eq!(inventory.active_session_count, 1);
        assert_eq!(inventory.terminal_session_count, 1);
        assert_eq!(inventory.pane_count, 1);
        assert_eq!(inventory.tab_count, 1);
        assert_eq!(inventory.status_counts["running"], 1);
        assert_eq!(inventory.status_counts["completed"], 1);
        assert_eq!(inventory.session_id_digests.len(), 2);

        let serialized = serde_json::to_string(&inventory).unwrap();
        assert!(!serialized.contains("session-secret"));
        assert!(!serialized.contains("SECRET_TITLE"));
        assert!(!serialized.contains("SECRET_PROMPT"));
        assert!(serialized.contains("sha256:"));
    }

    #[test]
    fn session_inventory_rejects_unknown_top_level_shapes() {
        let error = session_inventory_from_values(
            &json!({"unexpectedSessions": []}),
            &json!({"panes": []}),
        )
        .expect_err("unknown list_sessions shape must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("list_sessions"));

        let error = session_inventory_from_values(
            &json!({"sessions": []}),
            &json!({"unexpectedPanes": []}),
        )
        .expect_err("unknown list_panes shape must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("list_panes"));
    }

    #[test]
    fn session_inventory_rejects_non_object_entries() {
        let error = session_inventory_from_values(
            &json!({"sessions": ["not-a-session"]}),
            &json!({"panes": []}),
        )
        .expect_err("primitive session entries must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("list_sessions"));

        let error =
            session_inventory_from_values(&json!({"sessions": []}), &json!({"panes": [42]}))
                .expect_err("primitive pane entries must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("list_panes"));
    }

    #[test]
    fn session_inventory_rejects_malformed_tabs() {
        let error = session_inventory_from_values(
            &json!({"sessions": []}),
            &json!({"panes": [{"tabs": "not-an-array"}]}),
        )
        .expect_err("non-array tabs must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("tabs"));

        let error = session_inventory_from_values(
            &json!({"sessions": []}),
            &json!({"panes": [{"tabs": [null]}]}),
        )
        .expect_err("primitive tab entries must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("tabs"));
    }
}
