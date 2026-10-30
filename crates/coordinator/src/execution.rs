use std::thread;
use std::time::{Duration, Instant};

use paneforge_adapter::{CapabilitySnapshot, PaneForgeAdapter};
use coordination_core::{
    CoordinationError, ErrorCode, OutboxState, ResultAggregationState, WorkerState, WorkflowState,
};
use coordination_store::{
    ExternalBindingState, RecordExternalLaunchSuccessRequest, RecordWorkerResultRequest,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::CoordinatorRuntime;

pub const PANEFORGE_REAL_CANARY_CONTRACT_VERSION: &str = "coordination.paneforge-real-canary/v1beta1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneForgeCanaryPolicy {
    pub expected_marker: String,
    pub timeout: Duration,
    pub poll_interval: Duration,
}

impl PaneForgeCanaryPolicy {
    fn validate(&self) -> Result<(), CoordinationError> {
        if self.expected_marker.trim().is_empty() {
            return Err(CoordinationError::invalid_request(
                "canary expected marker must not be empty",
            ));
        }
        if self.timeout.is_zero() || self.poll_interval.is_zero() {
            return Err(CoordinationError::invalid_request(
                "canary timeout and poll interval must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaneForgeCanaryReport {
    pub contract_version: String,
    pub capability_readiness: String,
    pub dispatch_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_name: Option<String>,
    pub launch_workspace_name: String,
    pub expected_project_path_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_observed_digest: Option<String>,
    pub cwd_matched: bool,
    pub task_id_digest: String,
    pub session_id_digest: String,
    pub runtime_kind_digest: String,
    pub marker_observed: bool,
    pub final_status: String,
    pub final_status_is_accepted: bool,
    pub binding_state: ExternalBindingState,
    pub outbox_state: OutboxState,
    pub worker_state: WorkerState,
    pub workflow_state: WorkflowState,
    pub result_state: ResultAggregationState,
    pub launch_task_calls: u32,
    pub status_queries: u32,
    pub output_queries: u32,
    pub submit_calls: u32,
    pub kill_calls: u32,
    pub write_calls: u32,
}

impl CoordinatorRuntime {
    pub fn run_next_paneforge_canary(
        &mut self,
        adapter: &mut PaneForgeAdapter,
        capability: &CapabilitySnapshot,
        policy: PaneForgeCanaryPolicy,
        now_ms: i64,
    ) -> Result<Option<PaneForgeCanaryReport>, CoordinationError> {
        policy.validate()?;
        if !capability.dispatch_ready {
            return Err(CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                "PaneForge dispatch capability is not ready",
            ));
        }

        let plan = self.tick(now_ms)?;
        if plan.frozen {
            return Err(CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "Coordinator dispatch is frozen",
            ));
        }
        let Some(claimed) = self.claim_next_dispatch(now_ms)? else {
            return Ok(None);
        };
        let outbox_id = claimed.outbox.outbox_id.clone();
        let outbox_revision = claimed.outbox.revision;
        let outbox_generation = claimed.outbox.generation;
        let workflow_id = claimed.outbox.workflow_id.clone();
        let worker_id = claimed.outbox.worker_id.clone().ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "launch_task outbox is missing workerId",
            )
        })?;
        let binding = self
            .store()
            .external_binding_for_worker(&worker_id)?
            .ok_or_else(|| {
                CoordinationError::new(
                    ErrorCode::InternalInvariantViolation,
                    "dispatch worker is missing ExternalBinding",
                )
            })?;

        let launch_payload = claimed.outbox.payload.clone();
        let expected_project_path =
            required_payload_string(&launch_payload, "projectPath")?.to_owned();
        let launch_workspace_name =
            required_payload_string(&launch_payload, "workspaceName")?.to_owned();
        let source_workspace_name = launch_payload
            .get("sourceWorkspaceName")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);

        let launched = match adapter.launch_task(launch_payload, capability) {
            Ok(launched) => launched,
            Err(error) => {
                if error.code == ErrorCode::DispatchResultUnknown {
                    self.store_mut().record_dispatch_result_unknown(
                        &outbox_id,
                        outbox_revision,
                        &error.message,
                        "paneforge-real-canary-dispatch-unknown",
                    )?;
                }
                return Err(error);
            }
        };
        let launch_refs: Result<(String, String, String), CoordinationError> = (|| {
            Ok((
                require_launch_ref(launched.task_id, "taskId")?,
                require_launch_ref(launched.session_id, "sessionId")?,
                require_launch_ref(launched.runtime_kind, "runtimeKind")?,
            ))
        })();
        let (task_id, session_id, runtime_kind) = match launch_refs {
            Ok(launch_refs) => launch_refs,
            Err(error) => {
                self.store_mut().record_dispatch_result_unknown(
                    &outbox_id,
                    outbox_revision,
                    &error.message,
                    "paneforge-real-canary-launch-response-unknown",
                )?;
                return Err(error);
            }
        };

        self.store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id: binding.binding_id,
                expected_generation: outbox_generation,
                external_task_ref: Some(task_id.clone()),
                external_session_ref: session_id.clone(),
                correlation_id: "paneforge-real-canary-launch".to_owned(),
            })?;
        let dispatching = self.store().worker(&worker_id)?;
        self.store_mut().transition_worker_state(
            &worker_id,
            dispatching.revision,
            WorkerState::Running,
            "paneforge-real-canary-running",
        )?;
        self.store_mut().transition_outbox_state(
            &outbox_id,
            outbox_revision,
            OutboxState::Acknowledged,
            None,
            "paneforge-real-canary-launch-ack",
        )?;

        let started = Instant::now();
        let mut status_queries = 0_u32;
        let mut output_queries = 0_u32;
        let (final_status, marker_observed, final_status_is_accepted, output) = loop {
            let snapshot = adapter.reconcile_session(&session_id)?;
            status_queries = status_queries.saturating_add(1);
            output_queries = output_queries.saturating_add(1);
            let output = snapshot.output.unwrap_or_default();
            let marker_observed = output.contains(&policy.expected_marker);
            let final_status_is_accepted =
                snapshot.status == "idle" || (snapshot.terminal && snapshot.succeeded);
            if marker_observed && final_status_is_accepted {
                break (
                    snapshot.status,
                    marker_observed,
                    final_status_is_accepted,
                    output,
                );
            }
            if started.elapsed() >= policy.timeout {
                return Err(CoordinationError::new(
                    ErrorCode::ResultPending,
                    format!(
                        "PaneForge canary did not observe marker plus accepted final status within {} ms",
                        policy.timeout.as_millis()
                    ),
                ));
            }
            thread::sleep(policy.poll_interval);
        };
        let cwd_observed = extract_cwd_marker(&output);
        let cwd_matched = cwd_observed
            .as_deref()
            .is_some_and(|cwd| normalize_path(cwd) == normalize_path(&expected_project_path));
        if !cwd_matched {
            let running = self.store().worker(&worker_id)?;
            self.store_mut().transition_worker_state(
                &worker_id,
                running.revision,
                WorkerState::Failed,
                "paneforge-real-canary-cwd-mismatch",
            )?;
            let output_digest = digest(&output);
            self.store_mut()
                .record_worker_result(RecordWorkerResultRequest {
                    workflow_id: workflow_id.clone(),
                    worker_id: worker_id.clone(),
                    worker_state: WorkerState::Failed,
                    aggregation_state: ResultAggregationState::Partial,
                    summary: format!("canary cwd mismatch; output {output_digest}"),
                    artifact_sha256: Some(output_digest.trim_start_matches("sha256:").to_owned()),
                    truncated: false,
                    completed_at_ms: now_ms,
                    retention_until_ms: None,
                })?;
            self.advance_workflow(&workflow_id, now_ms)?;
            return Err(CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                "PaneForge canary cwd marker did not match target projectPath",
            )
            .with_current_state(if cwd_observed.is_some() {
                "CWD_MISMATCH"
            } else {
                "CWD_MARKER_MISSING"
            }));
        }

        let running = self.store().worker(&worker_id)?;
        self.store_mut().transition_worker_state(
            &worker_id,
            running.revision,
            WorkerState::Succeeded,
            "paneforge-real-canary-succeeded",
        )?;
        let output_digest = digest(&output);
        self.store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: format!("canary marker observed; output {output_digest}"),
                artifact_sha256: Some(output_digest.trim_start_matches("sha256:").to_owned()),
                truncated: false,
                completed_at_ms: now_ms,
                retention_until_ms: None,
            })?;
        self.advance_workflow(&workflow_id, now_ms)?;

        let binding = self
            .store()
            .external_binding_for_worker(&worker_id)?
            .expect("binding remains present");
        let outbox = self.store().outbox(&outbox_id)?;
        let worker = self.store().worker(&worker_id)?;
        let workflow = self.store().workflow(&workflow_id)?;
        let result = self.aggregate_workflow_result(&workflow_id)?;

        Ok(Some(PaneForgeCanaryReport {
            contract_version: PANEFORGE_REAL_CANARY_CONTRACT_VERSION.to_owned(),
            capability_readiness: format!("{:?}", capability.readiness).to_ascii_uppercase(),
            dispatch_ready: capability.dispatch_ready,
            source_workspace_name,
            launch_workspace_name,
            expected_project_path_digest: digest(&expected_project_path),
            cwd_observed_digest: cwd_observed.as_deref().map(digest),
            cwd_matched,
            task_id_digest: digest(&task_id),
            session_id_digest: digest(&session_id),
            runtime_kind_digest: digest(&runtime_kind),
            marker_observed,
            final_status,
            final_status_is_accepted,
            binding_state: binding.state,
            outbox_state: outbox.state,
            worker_state: worker.state,
            workflow_state: workflow.state,
            result_state: result.state,
            launch_task_calls: 1,
            status_queries,
            output_queries,
            submit_calls: 0,
            kill_calls: 0,
            write_calls: 0,
        }))
    }
}

fn require_launch_ref(
    value: Option<String>,
    field: &'static str,
) -> Result<String, CoordinationError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::DispatchResultUnknown,
                format!("PaneForge launch_task response is missing {field}"),
            )
        })
}

fn required_payload_string<'a>(
    payload: &'a serde_json::Value,
    field: &'static str,
) -> Result<&'a str, CoordinationError> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                format!("launch_task outbox payload is missing {field}"),
            )
        })
}

fn extract_cwd_marker(output: &str) -> Option<String> {
    const PREFIX: &str = "PANEFORGE_CANARY_CWD=";
    let lines = output.lines().collect::<Vec<_>>();
    let mut observed = None;
    for (index, line) in lines.iter().enumerate() {
        let Some(raw_value) = line.trim().strip_prefix(PREFIX) else {
            continue;
        };
        let mut value = raw_value.trim().to_owned();
        if !is_observed_cwd_marker(&value) {
            continue;
        }
        for continuation in lines.iter().skip(index + 1).take(4) {
            let continuation = continuation.trim();
            if !looks_like_wrapped_path_continuation(continuation) {
                break;
            }
            value.push_str(continuation);
        }
        if is_observed_cwd_marker(&value) {
            observed = Some(value);
        }
    }
    observed
}

fn is_observed_cwd_marker(value: &str) -> bool {
    !value.is_empty() && !value.contains("当前工作目录完整路径")
}

fn looks_like_wrapped_path_continuation(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value.chars().next(), Some('•' | '›' | '╭' | '╰' | '│'))
        && !value.contains("PANEFORGE_CANARY_CWD=")
        && (value.contains('\\') || value.contains('/'))
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn digest(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_launch_reference_is_dispatch_result_unknown() {
        let error = require_launch_ref(None, "sessionId").expect_err("missing launch ref");

        assert_eq!(error.code, ErrorCode::DispatchResultUnknown);
    }

    #[test]
    fn cwd_marker_parser_uses_worker_answer_not_echoed_prompt_placeholder() {
        let output = "\
PANEFORGE_CANARY_CWD=<当前工作目录完整路径>；不要复述原 challenge。
other terminal echo
PANEFORGE_CANARY_CWD=C:\\Temp\\synthetic-project
";

        assert_eq!(
            extract_cwd_marker(output).as_deref(),
            Some("C:\\Temp\\synthetic-project")
        );
    }

    #[test]
    fn cwd_marker_parser_ignores_prompt_placeholder_when_worker_answer_is_missing() {
        let output = "\
第二行格式为 PANEFORGE_CANARY_CWD=<当前工作目录完整路径>；不要复述原 challenge。
challenge:
PANEFORGE_CWD_ROUTE_20260811
";

        assert_eq!(extract_cwd_marker(output), None);
    }

    #[test]
    fn cwd_marker_parser_reassembles_terminal_wrapped_path() {
        let output = "\
• reversed-marker
  PANEFORGE_CANARY_CWD=C:\\Temp\\codex-paneforge-real-cwd-
  canary-20260811171002\\real-cwd-route-project

• hook
";

        assert_eq!(
            extract_cwd_marker(output).as_deref(),
            Some("C:\\Temp\\codex-paneforge-real-cwd-canary-20260811171002\\real-cwd-route-project")
        );
    }
}
