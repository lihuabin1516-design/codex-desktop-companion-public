use std::collections::BTreeSet;
use std::path::Path;

use coordination_core::{CoordinationError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::types::find_string;
use crate::{
    AdapterReadiness, AdapterStatusSnapshot, CcpanesAdapter, SanitizedEndpointConfig,
    CCPANES_ADAPTER_CONTRACT_VERSION, TOOL_ADD_PROJECT_TO_WORKSPACE, TOOL_CREATE_WORKSPACE,
    TOOL_LAUNCH_TASK, TOOL_LIST_PROJECTS, TOOL_LIST_WORKSPACES,
};

pub const DEDICATED_WORKSPACE_CONTRACT_VERSION: &str =
    "ccpanes-adapter-dedicated-workspace/v1beta1";
pub const DEDICATED_WORKSPACE_NAME_PREFIX: &str = "codex-ccpanes-phase7-";
const DEDICATED_WORKSPACE_DIGEST_HEX_LEN: usize = 24;

const READ_ONLY_WORKSPACE_OPERATIONS: &[&str] = &[
    "initialize",
    "notifications/initialized",
    "tools/list",
    TOOL_LIST_WORKSPACES,
    TOOL_LIST_PROJECTS,
];

const DEFERRED_SETUP_OPERATIONS: &[&str] = &[TOOL_CREATE_WORKSPACE, TOOL_ADD_PROJECT_TO_WORKSPACE];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DedicatedWorkspaceSpec {
    pub workspace_name: String,
    pub project_path: String,
    pub project_name: String,
}

impl DedicatedWorkspaceSpec {
    pub fn validate(&self) -> Result<(), CoordinationError> {
        if !self
            .workspace_name
            .starts_with(DEDICATED_WORKSPACE_NAME_PREFIX)
            || self.workspace_name.len() > 96
            || self.workspace_name.chars().any(char::is_control)
        {
            return Err(CoordinationError::invalid_request(format!(
                "workspaceName must start with {DEDICATED_WORKSPACE_NAME_PREFIX} and contain no control characters"
            )));
        }
        if self.project_name.trim().is_empty()
            || self.project_name.len() > 96
            || self.project_name.chars().any(char::is_control)
        {
            return Err(CoordinationError::invalid_request(
                "projectName must be non-empty and contain no control characters",
            ));
        }
        if self.project_path.trim().is_empty() || !Path::new(&self.project_path).is_absolute() {
            return Err(CoordinationError::invalid_request(
                "projectPath must be an absolute synthetic repository path",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DedicatedWorkspaceReadiness {
    Ready,
    RequiresSetup,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DedicatedWorkspaceProjectReport {
    pub contract_version: String,
    pub adapter_contract_version: String,
    pub readiness: DedicatedWorkspaceReadiness,
    pub endpoint: SanitizedEndpointConfig,
    pub read_only_operations: Vec<String>,
    pub deferred_setup_operations: Vec<String>,
    pub external_workspace_side_effects: bool,
    pub status: AdapterStatusSnapshot,
    pub launch_task_accepts_workspace_name: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<DedicatedWorkspaceSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<DedicatedProjectSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CoordinationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DedicatedWorkspaceSummary {
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DedicatedProjectSummary {
    pub project_name: String,
    pub project_path_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id_digest: Option<String>,
}

pub fn run_dedicated_workspace_project_gate(
    adapter: &mut CcpanesAdapter,
    spec: DedicatedWorkspaceSpec,
) -> DedicatedWorkspaceProjectReport {
    let endpoint = adapter.config().sanitized();
    if let Err(error) = spec.validate() {
        return report_with_error(
            endpoint,
            adapter.status_snapshot(),
            DedicatedWorkspaceReadiness::Incompatible,
            false,
            error,
        );
    }

    let capability = match adapter.status_snapshot().capability {
        Some(capability) => capability,
        None => match adapter.discover() {
            Ok(capability) => capability,
            Err(error) => {
                return report_with_error(
                    endpoint,
                    adapter.status_snapshot(),
                    readiness_for_error(&error),
                    false,
                    error,
                );
            }
        },
    };
    let launch_task_accepts_workspace_name = capability
        .launch_task_properties
        .iter()
        .any(|property| property == "workspaceName");
    if let Err(error) =
        require_workspace_capabilities(&capability.tools, launch_task_accepts_workspace_name)
    {
        let mut status = adapter.status_snapshot();
        status.readiness = AdapterReadiness::Incompatible;
        return report_with_error(
            endpoint,
            status,
            DedicatedWorkspaceReadiness::Incompatible,
            launch_task_accepts_workspace_name,
            error,
        );
    }

    let workspaces = match adapter.list_workspaces() {
        Ok(workspaces) => workspaces,
        Err(error) => {
            return report_with_error(
                endpoint,
                adapter.status_snapshot(),
                readiness_for_error(&error),
                launch_task_accepts_workspace_name,
                error,
            );
        }
    };
    if workspace_name_path_conflict(&spec, &workspaces) {
        let mut status = adapter.status_snapshot();
        status.readiness = AdapterReadiness::Incompatible;
        return report_with_error(
            endpoint,
            status,
            DedicatedWorkspaceReadiness::Incompatible,
            launch_task_accepts_workspace_name,
            CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                "dedicated workspace path does not match target projectPath",
            ),
        );
    }
    let projects = match adapter.list_projects() {
        Ok(projects) => projects,
        Err(error) => {
            return report_with_error(
                endpoint,
                adapter.status_snapshot(),
                readiness_for_error(&error),
                launch_task_accepts_workspace_name,
                error,
            );
        }
    };
    let (workspace, project) =
        workspace_project_summaries_from_values(&spec, &workspaces, &projects);
    let readiness = if workspace.is_some() && project.is_some() {
        DedicatedWorkspaceReadiness::Ready
    } else {
        DedicatedWorkspaceReadiness::RequiresSetup
    };

    DedicatedWorkspaceProjectReport {
        contract_version: DEDICATED_WORKSPACE_CONTRACT_VERSION.to_owned(),
        adapter_contract_version: CCPANES_ADAPTER_CONTRACT_VERSION.to_owned(),
        readiness,
        endpoint,
        read_only_operations: read_only_operations(),
        deferred_setup_operations: deferred_setup_operations(),
        external_workspace_side_effects: false,
        status: adapter.status_snapshot(),
        launch_task_accepts_workspace_name,
        workspace,
        project,
        error: None,
    }
}

pub fn ensure_dedicated_workspace_project(
    adapter: &mut CcpanesAdapter,
    spec: DedicatedWorkspaceSpec,
) -> DedicatedWorkspaceProjectReport {
    let initial = run_dedicated_workspace_project_gate(adapter, spec.clone());
    if initial.readiness != DedicatedWorkspaceReadiness::RequiresSetup {
        return initial;
    }

    let mut external_workspace_side_effects = false;
    if initial.workspace.is_none() {
        if let Err(error) = adapter.create_workspace(&spec.workspace_name, Some(&spec.project_path))
        {
            let mut report = report_with_error(
                adapter.config().sanitized(),
                adapter.status_snapshot(),
                readiness_for_error(&error),
                initial.launch_task_accepts_workspace_name,
                error,
            );
            report.external_workspace_side_effects = external_workspace_side_effects;
            return report;
        }
        external_workspace_side_effects = true;
    }
    if initial.project.is_none() {
        if let Err(error) =
            adapter.add_project_to_workspace(&spec.workspace_name, &spec.project_path)
        {
            let mut report = report_with_error(
                adapter.config().sanitized(),
                adapter.status_snapshot(),
                readiness_for_error(&error),
                initial.launch_task_accepts_workspace_name,
                error,
            );
            report.external_workspace_side_effects = external_workspace_side_effects;
            return report;
        }
        external_workspace_side_effects = true;
    }

    let mut verified = run_dedicated_workspace_project_gate(adapter, spec);
    verified.external_workspace_side_effects = external_workspace_side_effects;
    if verified.readiness != DedicatedWorkspaceReadiness::Ready && verified.error.is_none() {
        verified.error = Some(CoordinationError::new(
            ErrorCode::AdapterNotReady,
            "dedicated Workspace/Project setup did not converge to READY",
        ));
    }
    verified
}

pub fn dedicated_workspace_name_for_project_path(project_path: &str) -> String {
    let digest = hex::encode(Sha256::digest(normalize_path(project_path).as_bytes()));
    format!(
        "{DEDICATED_WORKSPACE_NAME_PREFIX}{}",
        &digest[..DEDICATED_WORKSPACE_DIGEST_HEX_LEN]
    )
}

pub fn workspace_project_summaries_from_values(
    spec: &DedicatedWorkspaceSpec,
    workspaces: &Value,
    projects: &Value,
) -> (
    Option<DedicatedWorkspaceSummary>,
    Option<DedicatedProjectSummary>,
) {
    let workspace = workspace_entries(workspaces)
        .into_iter()
        .find(|workspace| workspace_matches_spec(workspace, spec))
        .map(|workspace| DedicatedWorkspaceSummary {
            workspace_name: spec.workspace_name.clone(),
            workspace_id_digest: find_string(workspace, &["workspaceId", "workspace_id", "id"])
                .map(|id| digest_identifier(&id)),
            workspace_path_digest: find_string(
                workspace,
                &["path", "workspacePath", "workspace_path"],
            )
            .map(|path| digest_identifier(&path)),
        });
    let project = project_entries(projects)
        .into_iter()
        .find(|project| project_matches_spec(project, spec))
        .map(|project| DedicatedProjectSummary {
            project_name: find_string(project, &["name", "projectName", "project_name"])
                .unwrap_or_else(|| spec.project_name.clone()),
            project_path_digest: digest_identifier(&spec.project_path),
            project_id_digest: find_string(project, &["projectId", "project_id", "id"])
                .map(|id| digest_identifier(&id)),
        });

    (workspace, project)
}

fn require_workspace_capabilities(
    tools: &[String],
    launch_task_accepts_workspace_name: bool,
) -> Result<(), CoordinationError> {
    let tools = tools.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let missing = [
        TOOL_LIST_WORKSPACES,
        TOOL_LIST_PROJECTS,
        TOOL_CREATE_WORKSPACE,
        TOOL_ADD_PROJECT_TO_WORKSPACE,
    ]
    .into_iter()
    .filter(|tool| !tools.contains(*tool))
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            format!("CC-Panes workspace/project tools are missing: {missing:?}"),
        ));
    }
    if !tools.contains(TOOL_LAUNCH_TASK) || !launch_task_accepts_workspace_name {
        return Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            "CC-Panes launch_task must accept workspaceName for dedicated workspace canary",
        ));
    }
    Ok(())
}

fn workspace_entries(value: &Value) -> Vec<&Value> {
    value
        .get("workspaces")
        .and_then(Value::as_array)
        .or_else(|| value.get("items").and_then(Value::as_array))
        .or_else(|| value.as_array())
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn project_entries(value: &Value) -> Vec<&Value> {
    value
        .get("projects")
        .and_then(Value::as_array)
        .or_else(|| value.get("items").and_then(Value::as_array))
        .or_else(|| value.as_array())
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn workspace_matches_spec(workspace: &Value, spec: &DedicatedWorkspaceSpec) -> bool {
    let name_matches = find_string(workspace, &["name", "workspaceName", "workspace_name"])
        .as_deref()
        == Some(spec.workspace_name.as_str());
    let path_matches = find_string(workspace, &["path", "workspacePath", "workspace_path"])
        .is_none_or(|path| normalize_path(&path) == normalize_path(&spec.project_path));
    name_matches && path_matches
}

fn workspace_name_path_conflict(spec: &DedicatedWorkspaceSpec, workspaces: &Value) -> bool {
    workspace_entries(workspaces).into_iter().any(|workspace| {
        let name_matches = find_string(workspace, &["name", "workspaceName", "workspace_name"])
            .as_deref()
            == Some(spec.workspace_name.as_str());
        let Some(path) = find_string(workspace, &["path", "workspacePath", "workspace_path"])
        else {
            return false;
        };
        name_matches && normalize_path(&path) != normalize_path(&spec.project_path)
    })
}

fn project_matches_spec(project: &Value, spec: &DedicatedWorkspaceSpec) -> bool {
    let path_matches = find_string(project, &["path", "projectPath", "project_path"])
        .is_some_and(|path| normalize_path(&path) == normalize_path(&spec.project_path));
    let name_matches = find_string(project, &["name", "projectName", "project_name"]).as_deref()
        == Some(spec.project_name.as_str());
    let workspace_matches = find_string(project, &["workspace", "workspaceName", "workspace_name"])
        .as_deref()
        == Some(spec.workspace_name.as_str());
    path_matches && name_matches && workspace_matches
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

fn report_with_error(
    endpoint: SanitizedEndpointConfig,
    status: AdapterStatusSnapshot,
    readiness: DedicatedWorkspaceReadiness,
    launch_task_accepts_workspace_name: bool,
    error: CoordinationError,
) -> DedicatedWorkspaceProjectReport {
    DedicatedWorkspaceProjectReport {
        contract_version: DEDICATED_WORKSPACE_CONTRACT_VERSION.to_owned(),
        adapter_contract_version: CCPANES_ADAPTER_CONTRACT_VERSION.to_owned(),
        readiness,
        endpoint,
        read_only_operations: read_only_operations(),
        deferred_setup_operations: deferred_setup_operations(),
        external_workspace_side_effects: false,
        status,
        launch_task_accepts_workspace_name,
        workspace: None,
        project: None,
        error: Some(error),
    }
}

fn readiness_for_error(error: &CoordinationError) -> DedicatedWorkspaceReadiness {
    if error.code == ErrorCode::AdapterIncompatible {
        DedicatedWorkspaceReadiness::Incompatible
    } else {
        DedicatedWorkspaceReadiness::Degraded
    }
}

fn read_only_operations() -> Vec<String> {
    READ_ONLY_WORKSPACE_OPERATIONS
        .iter()
        .map(|operation| (*operation).to_owned())
        .collect()
}

fn deferred_setup_operations() -> Vec<String> {
    DEFERRED_SETUP_OPERATIONS
        .iter()
        .map(|operation| (*operation).to_owned())
        .collect()
}

fn digest_identifier(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn workspace_project_summary_redacts_ids_and_paths() {
        let spec = DedicatedWorkspaceSpec {
            workspace_name: "codex-ccpanes-phase7-fixture".to_owned(),
            project_path: "D:/SYNTHETIC_REPO_PATH".to_owned(),
            project_name: "phase7-project".to_owned(),
        };
        let (workspace, project) = workspace_project_summaries_from_values(
            &spec,
            &json!({"workspaces": [{"id": "workspace-secret-id", "name": spec.workspace_name}]}),
            &json!({"projects": [{
                "id": "project-secret-id",
                "name": "phase7-project",
                "path": "D:\\SYNTHETIC_REPO_PATH",
                "workspaceName": "codex-ccpanes-phase7-fixture"
            }]}),
        );

        let serialized = serde_json::to_string(&(workspace, project)).unwrap();
        assert!(!serialized.contains("workspace-secret-id"));
        assert!(!serialized.contains("project-secret-id"));
        assert!(!serialized.contains("SYNTHETIC_REPO_PATH"));
        assert!(serialized.contains("sha256:"));
    }

    #[test]
    fn dedicated_workspace_spec_rejects_non_dedicated_or_relative_project() {
        let bad_workspace = DedicatedWorkspaceSpec {
            workspace_name: "user-default".to_owned(),
            project_path: "D:/SYNTHETIC".to_owned(),
            project_name: "project".to_owned(),
        };
        assert_eq!(
            bad_workspace.validate().unwrap_err().code,
            ErrorCode::InvalidRequest
        );

        let bad_project = DedicatedWorkspaceSpec {
            workspace_name: "codex-ccpanes-phase7-fixture".to_owned(),
            project_path: "relative/project".to_owned(),
            project_name: "project".to_owned(),
        };
        assert_eq!(
            bad_project.validate().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }
}
