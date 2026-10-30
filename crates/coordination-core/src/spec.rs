use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CoordinationError, StepId};

pub const WORKFLOW_SPEC_SCHEMA_VERSION: &str = "coordination.workflow-spec/v1beta1";
pub const WORKFLOW_SPEC_KIND: &str = "WorkflowSpec";
pub const DEFAULT_MAX_PARALLEL_WORKERS: u32 = 12;
pub const DEFAULT_MAX_WORKERS: u32 = 64;
pub const MAX_PARALLEL_WORKERS_LIMIT: u32 = 256;
pub const MAX_WORKERS_LIMIT: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowSpecV1Beta1 {
    pub schema_version: String,
    pub kind: String,
    pub metadata: WorkflowMetadata,
    pub workflow: WorkflowDefinition,
}

impl WorkflowSpecV1Beta1 {
    pub fn validate(&self) -> Result<(), SpecValidationError> {
        let mut issues = Vec::new();
        if self.schema_version != WORKFLOW_SPEC_SCHEMA_VERSION {
            issues.push(issue(
                "schema_version_unsupported",
                "schemaVersion",
                format!("expected {WORKFLOW_SPEC_SCHEMA_VERSION}"),
            ));
        }
        if self.kind != WORKFLOW_SPEC_KIND {
            issues.push(issue(
                "invalid_request",
                "kind",
                format!("expected {WORKFLOW_SPEC_KIND}"),
            ));
        }
        required_text(&mut issues, "metadata.name", &self.metadata.name);
        required_text(&mut issues, "metadata.callerId", &self.metadata.caller_id);
        required_text(&mut issues, "workflow.objective", &self.workflow.objective);
        required_text(
            &mut issues,
            "workflow.repository.path",
            &self.workflow.repository.path,
        );
        optional_text_no_control(
            &mut issues,
            "workflow.repository.workspaceName",
            self.workflow.repository.workspace_name.as_deref(),
        );

        validate_scheduling(
            &self.workflow.scheduling,
            &self.workflow.budget,
            &mut issues,
        );
        if self.workflow.steps.len() > self.workflow.budget.max_workers as usize {
            issues.push(issue(
                "budget_exhausted",
                "workflow.budget.maxWorkers",
                "must cover the number of workflow steps",
            ));
        }
        validate_steps(&self.workflow.steps, &self.workflow.fan_in, &mut issues);

        if issues.is_empty() {
            Ok(())
        } else {
            Err(SpecValidationError { issues })
        }
    }

    pub fn require_supported_schema(&self) -> Result<(), CoordinationError> {
        if self.schema_version == WORKFLOW_SPEC_SCHEMA_VERSION {
            Ok(())
        } else {
            Err(CoordinationError::schema_version_unsupported(
                &self.schema_version,
                WORKFLOW_SPEC_SCHEMA_VERSION,
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowMetadata {
    pub name: String,
    pub caller_id: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowDefinition {
    pub objective: String,
    pub repository: RepositoryRef,
    #[serde(default)]
    pub scheduling: SchedulingSpec,
    #[serde(default)]
    pub budget: BudgetSpec,
    #[serde(default)]
    pub fan_in: FanInPolicy,
    pub steps: Vec<StepSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepositoryRef {
    pub path: String,
    #[serde(default)]
    pub expected_head: Option<String>,
    #[serde(default)]
    pub dirty_policy: DirtyPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DirtyPolicy {
    #[default]
    Reject,
    AllowDeclared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SchedulingSpec {
    #[serde(default = "default_max_parallel_workers")]
    pub max_parallel_workers: u32,
    #[serde(default = "default_one")]
    pub max_writer_workers_per_worktree: u32,
}

impl Default for SchedulingSpec {
    fn default() -> Self {
        Self {
            max_parallel_workers: default_max_parallel_workers(),
            max_writer_workers_per_worktree: default_one(),
        }
    }
}

fn default_max_parallel_workers() -> u32 {
    DEFAULT_MAX_PARALLEL_WORKERS
}

fn default_one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BudgetSpec {
    #[serde(default = "default_max_workers")]
    pub max_workers: u32,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_worker_log_bytes")]
    pub max_log_bytes_per_worker: u64,
    #[serde(default = "default_workflow_artifact_bytes")]
    pub max_artifact_bytes: u64,
}

impl Default for BudgetSpec {
    fn default() -> Self {
        Self {
            max_workers: default_max_workers(),
            timeout_seconds: default_timeout_seconds(),
            max_log_bytes_per_worker: default_worker_log_bytes(),
            max_artifact_bytes: default_workflow_artifact_bytes(),
        }
    }
}

fn default_max_workers() -> u32 {
    DEFAULT_MAX_WORKERS
}

fn default_timeout_seconds() -> u64 {
    14_400
}

fn default_worker_log_bytes() -> u64 {
    100 * 1024 * 1024
}

fn default_workflow_artifact_bytes() -> u64 {
    1024 * 1024 * 1024
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "policy", rename_all = "kebab-case")]
pub enum FanInPolicy {
    #[default]
    AllRequired,
    AllowPartial {
        #[serde(rename = "minSuccessful")]
        min_successful: u32,
    },
    FirstSuccess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StepSpec {
    pub step_id: StepId,
    pub objective: String,
    #[serde(default)]
    pub depends_on: Vec<StepId>,
    #[serde(default)]
    pub access: AccessMode,
    pub executor: ExecutorSpec,
    #[serde(default = "default_step_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_step_timeout_seconds() -> u64 {
    3_600
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExecutorSpec {
    #[serde(default = "default_cli_tool")]
    pub cli_tool: String,
}

fn default_cli_tool() -> String {
    "codex".to_owned()
}

impl Default for ExecutorSpec {
    fn default() -> Self {
        Self {
            cli_tool: default_cli_tool(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    #[default]
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecValidationIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecValidationError {
    pub issues: Vec<SpecValidationIssue>,
}

impl SpecValidationError {
    pub fn contains_code(&self, code: &str) -> bool {
        self.issues.iter().any(|issue| issue.code == code)
    }
}

impl fmt::Display for SpecValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workflow spec validation failed with {} issue(s)",
            self.issues.len()
        )
    }
}

impl std::error::Error for SpecValidationError {}

fn validate_scheduling(
    scheduling: &SchedulingSpec,
    budget: &BudgetSpec,
    issues: &mut Vec<SpecValidationIssue>,
) {
    if scheduling.max_parallel_workers == 0 {
        issues.push(issue(
            "invalid_request",
            "workflow.scheduling.maxParallelWorkers",
            "must be greater than zero",
        ));
    } else if scheduling.max_parallel_workers > MAX_PARALLEL_WORKERS_LIMIT {
        issues.push(issue(
            "capacity_exhausted",
            "workflow.scheduling.maxParallelWorkers",
            format!("must not exceed {MAX_PARALLEL_WORKERS_LIMIT}"),
        ));
    }
    if scheduling.max_writer_workers_per_worktree != 1 {
        issues.push(issue(
            "invalid_request",
            "workflow.scheduling.maxWriterWorkersPerWorktree",
            "must equal 1 for Coordination Plane v1beta1",
        ));
    }
    if budget.max_workers == 0 || budget.timeout_seconds == 0 {
        issues.push(issue(
            "budget_exhausted",
            "workflow.budget",
            "maxWorkers and timeoutSeconds must be positive",
        ));
    } else if budget.max_workers > MAX_WORKERS_LIMIT {
        issues.push(issue(
            "capacity_exhausted",
            "workflow.budget.maxWorkers",
            format!("must not exceed {MAX_WORKERS_LIMIT}"),
        ));
    }
    if scheduling.max_parallel_workers > budget.max_workers {
        issues.push(issue(
            "capacity_exhausted",
            "workflow.scheduling.maxParallelWorkers",
            "must not exceed workflow.budget.maxWorkers",
        ));
    }
}

fn validate_steps(steps: &[StepSpec], fan_in: &FanInPolicy, issues: &mut Vec<SpecValidationIssue>) {
    if steps.is_empty() {
        issues.push(issue(
            "invalid_request",
            "workflow.steps",
            "at least one step is required",
        ));
        return;
    }

    let mut ids = BTreeSet::new();
    for (index, step) in steps.iter().enumerate() {
        let path = format!("workflow.steps[{index}]");
        required_text(issues, &format!("{path}.objective"), &step.objective);
        required_text(
            issues,
            &format!("{path}.executor.cliTool"),
            &step.executor.cli_tool,
        );
        if !ids.insert(step.step_id.as_str().to_owned()) {
            issues.push(issue(
                "invalid_request",
                format!("{path}.stepId"),
                format!("duplicate step id {}", step.step_id),
            ));
        }
        if step.timeout_seconds == 0 {
            issues.push(issue(
                "invalid_request",
                format!("{path}.timeoutSeconds"),
                "must be greater than zero",
            ));
        }
    }

    for (index, step) in steps.iter().enumerate() {
        let path = format!("workflow.steps[{index}].dependsOn");
        for dependency in &step.depends_on {
            if dependency == &step.step_id {
                issues.push(issue(
                    "invalid_request",
                    &path,
                    "step must not depend on itself",
                ));
            } else if !ids.contains(dependency.as_str()) {
                issues.push(issue(
                    "invalid_request",
                    &path,
                    format!("unknown dependency {dependency}"),
                ));
            }
        }
    }

    let graph = steps
        .iter()
        .map(|step| (step.step_id.as_str(), step.depends_on.as_slice()))
        .collect::<HashMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for step in steps {
        if has_cycle(step.step_id.as_str(), &graph, &mut visiting, &mut visited) {
            issues.push(issue(
                "invalid_request",
                "workflow.steps",
                format!("cyclic dependency at {}", step.step_id),
            ));
            break;
        }
    }

    if let FanInPolicy::AllowPartial { min_successful } = fan_in {
        if *min_successful == 0 || *min_successful as usize > steps.len() {
            issues.push(issue(
                "invalid_request",
                "workflow.fanIn.minSuccessful",
                "must be between 1 and the number of steps",
            ));
        }
    }
}

fn has_cycle<'a>(
    id: &'a str,
    graph: &HashMap<&'a str, &'a [StepId]>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> bool {
    if visited.contains(id) {
        return false;
    }
    if !visiting.insert(id) {
        return true;
    }
    if let Some(dependencies) = graph.get(id) {
        for dependency in *dependencies {
            if has_cycle(dependency.as_str(), graph, visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(id);
    visited.insert(id);
    false
}

fn required_text(issues: &mut Vec<SpecValidationIssue>, path: &str, value: &str) {
    if value.trim().is_empty() {
        issues.push(issue("invalid_request", path, "must not be empty"));
    }
}

fn optional_text_no_control(
    issues: &mut Vec<SpecValidationIssue>,
    path: &str,
    value: Option<&str>,
) {
    let Some(value) = value else {
        return;
    };
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        issues.push(issue(
            "invalid_request",
            path,
            "must be non-empty and contain no control characters",
        ));
    }
}

fn issue(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> SpecValidationIssue {
    SpecValidationIssue {
        code: code.into(),
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    fn step(id: &str) -> StepSpec {
        StepSpec {
            step_id: StepId::try_new(id).expect("step id"),
            objective: format!("do {id}"),
            depends_on: Vec::new(),
            access: AccessMode::Read,
            executor: ExecutorSpec::default(),
            timeout_seconds: 120,
            required: true,
        }
    }

    fn valid_spec() -> WorkflowSpecV1Beta1 {
        WorkflowSpecV1Beta1 {
            schema_version: WORKFLOW_SPEC_SCHEMA_VERSION.to_owned(),
            kind: WORKFLOW_SPEC_KIND.to_owned(),
            metadata: WorkflowMetadata {
                name: "phase1".to_owned(),
                caller_id: "codex-app".to_owned(),
                labels: BTreeMap::new(),
            },
            workflow: WorkflowDefinition {
                objective: "verify contracts".to_owned(),
                repository: RepositoryRef {
                    path: "D:/repo".to_owned(),
                    expected_head: Some("HEAD".to_owned()),
                    dirty_policy: DirtyPolicy::Reject,
                    workspace_name: None,
                },
                scheduling: SchedulingSpec::default(),
                budget: BudgetSpec::default(),
                fan_in: FanInPolicy::AllRequired,
                steps: vec![step("read")],
            },
        }
    }

    #[test]
    fn workflow_spec_v1beta1_accepts_valid_contract() {
        assert_eq!(valid_spec().validate(), Ok(()));
    }

    #[test]
    fn workflow_spec_rejects_unknown_schema_version() {
        let mut spec = valid_spec();
        spec.schema_version = "coordination.workflow-spec/v9".to_owned();
        let errors = spec.validate().expect_err("unknown version");
        assert!(errors.contains_code("schema_version_unsupported"));
        assert_eq!(
            spec.require_supported_schema()
                .expect_err("unknown schema")
                .code,
            ErrorCode::SchemaVersionUnsupported
        );
    }

    #[test]
    fn workflow_spec_rejects_duplicate_step_ids() {
        let mut spec = valid_spec();
        spec.workflow.steps.push(step("read"));
        let errors = spec.validate().expect_err("duplicate step");
        assert!(errors.contains_code("invalid_request"));
        assert!(errors
            .issues
            .iter()
            .any(|issue| issue.message.contains("duplicate step id")));
    }

    #[test]
    fn workflow_spec_rejects_unknown_dependency_and_cycles() {
        let mut unknown = valid_spec();
        unknown.workflow.steps[0].depends_on = vec![StepId::try_new("missing").unwrap()];
        let unknown_errors = unknown.validate().expect_err("unknown dependency");
        assert!(unknown_errors
            .issues
            .iter()
            .any(|issue| issue.message.contains("unknown dependency")));

        let mut cyclic = valid_spec();
        cyclic.workflow.steps.push(step("write"));
        cyclic.workflow.steps[0].depends_on = vec![StepId::try_new("write").unwrap()];
        cyclic.workflow.steps[1].depends_on = vec![StepId::try_new("read").unwrap()];
        let cyclic_errors = cyclic.validate().expect_err("cycle");
        assert!(cyclic_errors
            .issues
            .iter()
            .any(|issue| issue.message.contains("cyclic dependency")));
    }

    #[test]
    fn workflow_spec_rejects_capacity_and_partial_policy_mismatches() {
        let mut spec = valid_spec();
        spec.workflow.scheduling.max_parallel_workers = 3;
        spec.workflow.budget.max_workers = 2;
        spec.workflow.fan_in = FanInPolicy::AllowPartial { min_successful: 2 };
        let errors = spec.validate().expect_err("capacity mismatch");
        assert!(errors.contains_code("capacity_exhausted"));
        assert!(errors
            .issues
            .iter()
            .any(|issue| issue.path == "workflow.fanIn.minSuccessful"));
    }

    #[test]
    fn workflow_spec_phase6_accepts_twelve_read_only_steps_and_fan_in_policies() {
        let mut spec = valid_spec();
        spec.workflow.scheduling.max_parallel_workers = 12;
        spec.workflow.budget.max_workers = 12;
        spec.workflow.steps = (1..=12)
            .map(|index| step(&format!("step-{index:02}")))
            .collect();
        spec.workflow.fan_in = FanInPolicy::AllowPartial { min_successful: 8 };
        assert_eq!(spec.validate(), Ok(()));

        spec.workflow.fan_in = FanInPolicy::FirstSuccess;
        assert_eq!(spec.validate(), Ok(()));
    }

    #[test]
    fn workflow_spec_fan_in_policy_uses_public_camel_case_fields() {
        let mut spec = valid_spec();
        spec.workflow.fan_in = FanInPolicy::AllowPartial { min_successful: 1 };
        let value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["workflow"]["fanIn"]["policy"], "allow-partial");
        assert_eq!(value["workflow"]["fanIn"]["minSuccessful"], 1);
        assert!(value["workflow"]["fanIn"].get("min_successful").is_none());
    }

    #[test]
    fn workflow_spec_rejects_step_count_over_max_workers_budget() {
        let mut spec = valid_spec();
        spec.workflow.budget.max_workers = 1;
        spec.workflow.steps.push(step("second"));
        let errors = spec
            .validate()
            .expect_err("step count exceeds worker budget");
        assert!(errors.contains_code("budget_exhausted"));
        assert!(errors
            .issues
            .iter()
            .any(|issue| issue.path == "workflow.budget.maxWorkers"));
    }

    #[test]
    fn workflow_spec_accepts_and_validates_repository_workspace_name() {
        let mut spec = valid_spec();
        spec.workflow.repository.workspace_name = Some("codex-ccpanes-phase7-success".to_owned());
        assert_eq!(spec.validate(), Ok(()));
        let value = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            value["workflow"]["repository"]["workspaceName"],
            "codex-ccpanes-phase7-success"
        );

        spec.workflow.repository.workspace_name = Some("bad\nname".to_owned());
        let errors = spec.validate().expect_err("bad workspace name");
        assert!(errors
            .issues
            .iter()
            .any(|issue| issue.path == "workflow.repository.workspaceName"));
    }
}
