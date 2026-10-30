use std::collections::BTreeSet;

use coordination_core::{CoordinationError, ErrorCode};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::types::find_string;
use crate::{DiscoveredSessionBinding, DEFAULT_RECONCILE_WINDOW_MS};

pub fn match_unknown_dispatch(
    session_listing: &Value,
    pane_listing: &Value,
    client_request_id: &str,
    title_marker: &str,
    prompt_hash: &str,
    created_at_ms: i64,
) -> Result<Option<DiscoveredSessionBinding>, CoordinationError> {
    let online_session_ids = collect_session_ids(session_listing);
    let mut candidates = Vec::new();
    collect_session_candidates(session_listing, &mut candidates);
    collect_session_candidates(pane_listing, &mut candidates);

    let mut matches = Vec::new();
    for candidate in candidates {
        let Some(session_id) = find_string(
            candidate,
            &["sessionId", "session_id", "ptySessionId", "pty_session_id"],
        ) else {
            continue;
        };
        if !online_session_ids.contains(&session_id) {
            continue;
        }
        let request_match = !client_request_id.is_empty()
            && value_contains_exact_string(candidate, client_request_id);
        let marker_match =
            !title_marker.is_empty() && value_contains_string(candidate, title_marker);
        let prompt_match = !prompt_hash.is_empty()
            && candidate_within_creation_window(candidate, created_at_ms)
            && value_contains_prompt_hash(candidate, prompt_hash, title_marker);
        let score = if request_match {
            3_u8
        } else if marker_match {
            2_u8
        } else if prompt_match {
            1_u8
        } else {
            0_u8
        };
        if score == 0 {
            continue;
        }
        matches.push((
            score,
            DiscoveredSessionBinding {
                task_id: find_string(candidate, &["taskId", "task_id"]),
                session_id,
                binding_id: find_string(candidate, &["bindingId", "binding_id"]),
                match_basis: if request_match {
                    "clientRequestId".to_owned()
                } else if marker_match {
                    "titleMarker".to_owned()
                } else {
                    "promptHash+creationWindow".to_owned()
                },
            },
        ));
    }
    let Some(highest_score) = matches.iter().map(|(score, _)| *score).max() else {
        return Ok(None);
    };
    let mut strongest = matches
        .into_iter()
        .filter_map(|(score, binding)| (score == highest_score).then_some(binding))
        .collect::<Vec<_>>();
    strongest.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    strongest.dedup_by(|left, right| left.session_id == right.session_id);
    if strongest.len() > 1 {
        return Err(CoordinationError::new(
            ErrorCode::ExternalBindingConflict,
            format!("CC-Panes reconciliation found multiple sessions for marker {title_marker}"),
        ));
    }
    Ok(strongest.pop())
}

pub fn collect_session_ids(listing: &Value) -> BTreeSet<String> {
    let mut candidates = Vec::new();
    collect_session_candidates(listing, &mut candidates);
    candidates
        .into_iter()
        .filter_map(|candidate| {
            find_string(
                candidate,
                &["sessionId", "session_id", "ptySessionId", "pty_session_id"],
            )
        })
        .collect()
}

pub fn candidate_within_creation_window(candidate: &Value, expected_ms: i64) -> bool {
    find_i64(
        candidate,
        &[
            "createdAtMs",
            "created_at_ms",
            "startedAtMs",
            "started_at_ms",
            "createdAt",
            "created_at",
            "startedAt",
            "started_at",
        ],
    )
    .map(|actual_ms| actual_ms.saturating_sub(expected_ms).abs() <= DEFAULT_RECONCILE_WINDOW_MS)
    .unwrap_or(false)
}

pub fn collect_session_candidates<'a>(value: &'a Value, candidates: &mut Vec<&'a Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_session_candidates(value, candidates);
            }
        }
        Value::Object(values) => {
            let has_session_id = ["sessionId", "session_id", "ptySessionId", "pty_session_id"]
                .iter()
                .any(|name| values.get(*name).and_then(Value::as_str).is_some());
            if has_session_id {
                candidates.push(value);
            } else {
                for value in values.values() {
                    collect_session_candidates(value, candidates);
                }
            }
        }
        _ => {}
    }
}

fn value_contains_prompt_hash(value: &Value, expected_hash: &str, title_marker: &str) -> bool {
    match value {
        Value::String(value) => string_matches_prompt_hash(value, expected_hash, title_marker),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_prompt_hash(value, expected_hash, title_marker)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_prompt_hash(value, expected_hash, title_marker)),
        _ => false,
    }
}

fn string_matches_prompt_hash(value: &str, expected_hash: &str, title_marker: &str) -> bool {
    let trimmed = value.trim();
    let without_marker = trimmed
        .strip_prefix(title_marker)
        .map(str::trim)
        .unwrap_or(trimmed);
    let without_any_coordination_marker = if trimmed.starts_with('[') {
        trimmed
            .split_once('\n')
            .map(|(_, prompt)| prompt.trim())
            .unwrap_or(trimmed)
    } else {
        trimmed
    };
    [trimmed, without_marker, without_any_coordination_marker]
        .iter()
        .any(|candidate| hex::encode(Sha256::digest(candidate.as_bytes())) == expected_hash)
}

fn value_contains_exact_string(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_exact_string(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_exact_string(value, expected)),
        _ => false,
    }
}

fn value_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value.contains(needle),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_string(value, needle)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_string(value, needle)),
        _ => false,
    }
}

fn find_i64(value: &Value, names: &[&str]) -> Option<i64> {
    for name in names {
        if let Some(found) = value.get(*name).and_then(Value::as_i64) {
            return Some(found);
        }
        if let Some(found) = value.get(*name).and_then(Value::as_str) {
            if let Ok(number) = found.parse::<i64>() {
                return Some(number);
            }
        }
    }
    value
        .as_object()
        .into_iter()
        .flat_map(Map::values)
        .find_map(|nested| {
            nested
                .is_object()
                .then(|| find_i64(nested, names))
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn unknown_dispatch_prefers_client_request_id_over_marker() {
        let listing = json!({
            "sessions": [
                {"sessionId": "marker-only", "title": "[CODEX-CCPANES:run:step:1] worker"},
                {
                    "sessionId": "request-match",
                    "taskId": "task-1",
                    "metadata": {"clientRequestId": "request-1"},
                    "title": "[CODEX-CCPANES:run:step:1] worker"
                }
            ]
        });
        let binding = match_unknown_dispatch(
            &listing,
            &json!({"panes": []}),
            "request-1",
            "[CODEX-CCPANES:run:step:1]",
            "unused",
            1_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(binding.session_id, "request-match");
        assert_eq!(binding.task_id.as_deref(), Some("task-1"));
        assert_eq!(binding.match_basis, "clientRequestId");
    }

    #[test]
    fn unknown_dispatch_rejects_ambiguous_marker_matches() {
        let sessions = json!({"sessions": [{"sessionId": "one"}, {"sessionId": "two"}]});
        let panes = json!({
            "layouts": [{"panes": [{"tabs": [
                {"sessionId": "one", "title": "[CODEX-CCPANES:run:step:1] worker"},
                {"sessionId": "two", "title": "[CODEX-CCPANES:run:step:1] worker"}
            ]}]}]
        });
        let error = match_unknown_dispatch(
            &sessions,
            &panes,
            "missing",
            "[CODEX-CCPANES:run:step:1]",
            "unused",
            1_000,
        )
        .expect_err("ambiguous");
        assert_eq!(error.code, ErrorCode::ExternalBindingConflict);
    }

    #[test]
    fn unknown_dispatch_uses_prompt_hash_only_inside_creation_window() {
        let prompt_hash = hex::encode(Sha256::digest(b"produce result"));
        let sessions = json!({"sessions": [{"sessionId": "prompt-match", "createdAtMs": 1_000}]});
        let panes = json!({
            "layouts": [{"panes": [{"tabs": [{
                "sessionId": "prompt-match",
                "prompt": "[CODEX-CCPANES:different]\nproduce result",
                "createdAtMs": 1_000
            }]}]}]
        });
        let binding = match_unknown_dispatch(
            &sessions,
            &panes,
            "missing",
            "[CODEX-CCPANES:run:step:1]",
            &prompt_hash,
            1_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(binding.session_id, "prompt-match");
        assert_eq!(binding.match_basis, "promptHash+creationWindow");

        assert!(match_unknown_dispatch(
            &sessions,
            &panes,
            "missing",
            "[CODEX-CCPANES:run:step:1]",
            &prompt_hash,
            60 * 60 * 1_000,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn unknown_dispatch_deduplicates_current_layout_view() {
        let sessions = json!({"sessions": [{"sessionId": "session-1", "status": "working"}]});
        let tab = json!({"sessionId": "session-1", "title": "[CODEX-CCPANES:run:step:1] worker"});
        let panes =
            json!({"panes": [{"tabs": [tab.clone()]}], "layouts": [{"panes": [{"tabs": [tab]}]}]});
        let binding = match_unknown_dispatch(
            &sessions,
            &panes,
            "missing",
            "[CODEX-CCPANES:run:step:1]",
            "unused",
            1_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(binding.session_id, "session-1");
    }

    #[test]
    fn unknown_dispatch_ignores_offline_pane_tab() {
        let sessions = json!({"sessions": [{"sessionId": "other", "status": "working"}]});
        let panes = json!({"layouts": [{"panes": [{"tabs": [{"sessionId": "stale", "title": "[CODEX-CCPANES:run:step:1] worker"}]}]}]});
        let binding = match_unknown_dispatch(
            &sessions,
            &panes,
            "missing",
            "[CODEX-CCPANES:run:step:1]",
            "unused",
            1_000,
        )
        .unwrap();
        assert!(binding.is_none());
    }
}
