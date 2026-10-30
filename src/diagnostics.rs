use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::{default_config_path, load_config};
use crate::discovery::{external_command_path, resolve_ccpanes, resolve_codex};
use crate::environment::{current_sanitized_keys, sanitize_command};
use crate::hashing::{hash_file, hash_path};
use crate::logging::unix_ms;
use crate::PRODUCT_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn rank(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Warn => 1,
            Self::Fail => 2,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub schema_version: u32,
    pub product_version: &'static str,
    pub generated_unix_ms: u128,
    pub overall: CheckStatus,
    pub checks: Vec<DiagnosticCheck>,
}

impl DoctorReport {
    pub fn exit_code(&self) -> i32 {
        match self.overall {
            CheckStatus::Pass => 0,
            CheckStatus::Warn => 1,
            CheckStatus::Fail => 2,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub schema_version: u32,
    pub available: bool,
    pub source: String,
    pub path_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub bytes: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_version: Option<String>,
}

#[derive(Debug, Default)]
pub struct DoctorOptions {
    pub explicit_target: Option<PathBuf>,
    pub show_paths: bool,
}

pub fn probe(explicit: Option<&Path>, show_paths: bool) -> Result<ProbeReport> {
    let target = resolve_ccpanes(explicit)?;
    let metadata = fs::metadata(&target.path)
        .with_context(|| format!("read CC-Panes metadata ({})", target.source))?;
    Ok(ProbeReport {
        schema_version: 1,
        available: true,
        source: target.source,
        path_hash: hash_path(&target.path),
        path: show_paths.then(|| external_command_path(&target.path).display().to_string()),
        bytes: metadata.len(),
        sha256: hash_file(&target.path)?,
        product_version: windows_product_version(&target.path).ok().flatten(),
    })
}

pub fn doctor(options: DoctorOptions) -> DoctorReport {
    let mut checks = Vec::new();
    let mut ccpanes_version = None;

    match probe(options.explicit_target.as_deref(), options.show_paths) {
        Ok(report) => {
            ccpanes_version.clone_from(&report.product_version);
            let mut details = BTreeMap::new();
            details.insert("source".into(), json!(report.source));
            details.insert("pathHash".into(), json!(report.path_hash));
            details.insert("bytes".into(), json!(report.bytes));
            details.insert("sha256".into(), json!(report.sha256));
            if let Some(path) = report.path {
                details.insert("path".into(), json!(path));
            }
            if let Some(version) = report.product_version {
                details.insert("productVersion".into(), json!(version));
            }
            checks.push(check(
                "ccpanes_installation",
                CheckStatus::Pass,
                "发现官方 CC-Panes 安装",
                details,
            ));
        }
        Err(error) => checks.push(check(
            "ccpanes_installation",
            CheckStatus::Fail,
            format!("未发现可用的官方 CC-Panes：{error:#}"),
            BTreeMap::new(),
        )),
    }

    match resolve_codex() {
        Ok(codex) => {
            let mut details = BTreeMap::new();
            details.insert("source".into(), json!(codex.source));
            details.insert("pathHash".into(), json!(hash_path(&codex.path)));
            if options.show_paths {
                details.insert(
                    "path".into(),
                    json!(external_command_path(&codex.path).display().to_string()),
                );
            }
            match codex_version(&codex.path) {
                Ok(Some(version)) => {
                    details.insert("version".into(), json!(version));
                    checks.push(check(
                        "codex_cli",
                        CheckStatus::Pass,
                        "Codex CLI 可发现并可执行",
                        details,
                    ));
                }
                Ok(None) => checks.push(check(
                    "codex_cli",
                    CheckStatus::Warn,
                    "Codex CLI 已发现，但没有返回版本",
                    details,
                )),
                Err(error) => {
                    details.insert("probeError".into(), json!(format!("{error:#}")));
                    checks.push(check(
                        "codex_cli",
                        CheckStatus::Warn,
                        "Codex CLI 已发现，但版本探测失败",
                        details,
                    ));
                }
            }
        }
        Err(error) => checks.push(check(
            "codex_cli",
            CheckStatus::Fail,
            format!("未发现 Codex CLI：{error:#}"),
            BTreeMap::new(),
        )),
    }

    let inherited = current_sanitized_keys()
        .into_iter()
        .map(|key| key.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut environment_details = BTreeMap::new();
    environment_details.insert("detectedKeys".into(), json!(inherited));
    let environment_status = if inherited.is_empty() {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    };
    let environment_message = if inherited.is_empty() {
        "当前父进程没有 Codex Desktop 任务级私有环境"
    } else {
        "检测到任务级私有环境；Companion 启动 CC-Panes 时会精确移除"
    };
    checks.push(check(
        "parent_environment",
        environment_status,
        environment_message,
        environment_details,
    ));

    match process_names() {
        Ok(processes) => {
            let ccpanes_count = processes
                .iter()
                .filter(|name| name.starts_with("cc-panes"))
                .count();
            let build = processes
                .iter()
                .filter(|name| {
                    matches!(name.as_str(), "cargo.exe" | "rustc.exe" | "cargo" | "rustc")
                })
                .cloned()
                .collect::<Vec<_>>();
            let legacy = processes
                .iter()
                .filter(|name| {
                    matches!(
                        name.as_str(),
                        "liveagent-workflow-mcp.exe"
                            | "liveagent-workflow-mcp"
                            | "workflowd.exe"
                            | "workflowd"
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            let mut details = BTreeMap::new();
            details.insert("ccpanesProcessCount".into(), json!(ccpanes_count));
            details.insert("buildProcesses".into(), json!(build));
            details.insert("legacyWorkflowProcesses".into(), json!(legacy));
            let status = if build.is_empty() && legacy.is_empty() {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            };
            checks.push(check(
                "process_state",
                status,
                if status == CheckStatus::Pass {
                    "未发现 Rust 构建或遗留 Workflow 进程"
                } else {
                    "发现构建或遗留 Workflow 进程，请确认是否符合预期"
                },
                details,
            ));
        }
        Err(error) => {
            let mut details = BTreeMap::new();
            details.insert("probeError".into(), json!(format!("{error:#}")));
            checks.push(check(
                "process_state",
                CheckStatus::Warn,
                "进程状态探测失败",
                details,
            ));
        }
    }

    checks.push(legacy_mcp_check());
    checks.push(companion_config_check(options.show_paths));
    checks.push(compatibility_check(ccpanes_version.as_deref()));

    let overall = checks
        .iter()
        .map(|item| item.status)
        .max_by_key(|status| status.rank())
        .unwrap_or(CheckStatus::Pass);
    DoctorReport {
        schema_version: 1,
        product_version: PRODUCT_VERSION,
        generated_unix_ms: unix_ms(),
        overall,
        checks,
    }
}

fn companion_config_check(show_paths: bool) -> DiagnosticCheck {
    let path = default_config_path();
    let mut details = BTreeMap::new();
    details.insert("configPathHash".into(), json!(hash_path(&path)));
    if show_paths {
        details.insert("configPath".into(), json!(path.display().to_string()));
    }
    if !path.is_file() {
        return check(
            "companion_config",
            CheckStatus::Pass,
            "未创建可选 Companion 配置；当前使用自动发现",
            details,
        );
    }
    if let Ok(metadata) = fs::metadata(&path) {
        details.insert("configBytes".into(), json!(metadata.len()));
    }
    if let Ok(hash) = hash_file(&path) {
        details.insert("configSha256".into(), json!(hash));
    }
    match load_config(&path) {
        Ok(Some(config)) => {
            details.insert("schemaVersion".into(), json!(config.schema_version));
            details.insert("source".into(), json!(config.source));
            details.insert(
                "ccpanesPathHash".into(),
                json!(hash_path(&config.ccpanes_executable)),
            );
            check(
                "companion_config",
                CheckStatus::Pass,
                "Companion 配置有效",
                details,
            )
        }
        Ok(None) => check(
            "companion_config",
            CheckStatus::Pass,
            "未创建可选 Companion 配置；当前使用自动发现",
            details,
        ),
        Err(error) => {
            details.insert("probeError".into(), json!(format!("{error:#}")));
            check(
                "companion_config",
                CheckStatus::Warn,
                "Companion 配置存在但无效；当前将回退自动发现",
                details,
            )
        }
    }
}

fn compatibility_check(version: Option<&str>) -> DiagnosticCheck {
    let mut details = BTreeMap::new();
    details.insert("minimumRecommended".into(), json!("0.11.3"));
    details.insert("maximumPinned".into(), Value::Null);
    details.insert("policy".into(), json!("update-transparent"));
    match version.and_then(parse_version) {
        Some(version_tuple) if version_tuple >= (0, 11, 3) => check(
            "compatibility",
            CheckStatus::Pass,
            "CC-Panes 版本满足最低建议基线；未来版本不设上限",
            details,
        ),
        Some(_) => check(
            "compatibility",
            CheckStatus::Warn,
            "CC-Panes 版本低于最低建议基线 0.11.3",
            details,
        ),
        None => check(
            "compatibility",
            CheckStatus::Warn,
            "没有取得 CC-Panes 产品版本；启动仍按前向兼容策略放行",
            details,
        ),
    }
}

fn legacy_mcp_check() -> DiagnosticCheck {
    let Some(home) = env::var_os("USERPROFILE").map(PathBuf::from) else {
        return check(
            "legacy_workflow_mcp",
            CheckStatus::Pass,
            "未发现用户配置根；Companion 本身不依赖 LiveAgent MCP",
            BTreeMap::new(),
        );
    };
    let path = home.join(".codex").join("config.toml");
    if !path.is_file() {
        return check(
            "legacy_workflow_mcp",
            CheckStatus::Pass,
            "未注册用户级 Codex 配置；Companion 本身不依赖 LiveAgent MCP",
            BTreeMap::new(),
        );
    }
    let mut details = BTreeMap::new();
    if let Ok(metadata) = fs::metadata(&path) {
        details.insert("configBytes".into(), json!(metadata.len()));
    }
    if let Ok(hash) = hash_file(&path) {
        details.insert("configSha256".into(), json!(hash));
    }
    match parse_section_enabled(&path, "mcp_servers.liveagent_workflow") {
        Ok(Some(false)) => check(
            "legacy_workflow_mcp",
            CheckStatus::Pass,
            "遗留 liveagent_workflow MCP 已禁用",
            details,
        ),
        Ok(Some(true)) => check(
            "legacy_workflow_mcp",
            CheckStatus::Warn,
            "遗留 liveagent_workflow MCP 仍启用，但 Companion 不需要它",
            details,
        ),
        Ok(None) => check(
            "legacy_workflow_mcp",
            CheckStatus::Pass,
            "未注册遗留 liveagent_workflow MCP",
            details,
        ),
        Err(error) => {
            details.insert("probeError".into(), json!(format!("{error:#}")));
            check(
                "legacy_workflow_mcp",
                CheckStatus::Warn,
                "Codex 配置存在，但遗留 MCP 状态未解析",
                details,
            )
        }
    }
}

fn parse_section_enabled(path: &Path, section: &str) -> Result<Option<bool>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read configuration {}", path.display()))?;
    let header = format!("[{section}]");
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section {
                break;
            }
            in_section = trimmed == header;
            continue;
        }
        if !in_section || trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("enabled") {
            let Some(value) = value.trim_start().strip_prefix('=') else {
                continue;
            };
            return match value.trim().split('#').next().unwrap_or("").trim() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Ok(None),
            };
        }
    }
    Ok(None)
}

fn process_names() -> Result<Vec<String>> {
    #[cfg(windows)]
    let output = Command::new("tasklist.exe")
        .args(["/fo", "csv", "/nh"])
        .output()
        .context("run tasklist")?;
    #[cfg(not(windows))]
    let output = Command::new("ps")
        .args(["-eo", "comm="])
        .output()
        .context("run ps")?;

    if !output.status.success() {
        anyhow::bail!("process_probe_exit_{}", output.status);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    #[cfg(windows)]
    let names = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix('"')
                .and_then(|rest| rest.find('"').map(|index| &rest[..index]))
        })
        .map(|name| name.to_ascii_lowercase())
        .collect();
    #[cfg(not(windows))]
    let names = stdout
        .lines()
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    Ok(names)
}

fn codex_version(path: &Path) -> Result<Option<String>> {
    let command_path = external_command_path(path);
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "& $env:CODEX_CCPANES_CODEX_PROBE --version",
            ])
            .env("CODEX_CCPANES_CODEX_PROBE", &command_path);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(&command_path);
        command.arg("--version");
        command
    };
    let sanitized = current_sanitized_keys();
    sanitize_command(&mut command, &sanitized);
    let output = command.output().context("run codex --version")?;
    if !output.status.success() {
        anyhow::bail!("codex_version_exit_{}", output.status);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

fn windows_product_version(path: &Path) -> Result<Option<String>> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$v=(Get-Item -LiteralPath $env:CODEX_CCPANES_VERSION_PROBE).VersionInfo.ProductVersion; if($v){$v}",
            ])
            .env("CODEX_CCPANES_VERSION_PROBE", path)
            .output()
            .context("probe Windows product version")?;
        if !output.status.success() {
            anyhow::bail!("product_version_probe_exit_{}", output.status);
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((!value.is_empty()).then_some(value))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(None)
    }
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let normalized = value.trim().trim_start_matches('v');
    let mut parts = normalized.split(['.', '-', '+']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn check(
    id: impl Into<String>,
    status: CheckStatus,
    message: impl Into<String>,
    details: BTreeMap<String, Value>,
) -> DiagnosticCheck {
    DiagnosticCheck {
        id: id.into(),
        status,
        message: message.into(),
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_with_suffix() {
        assert_eq!(parse_version("0.11.10"), Some((0, 11, 10)));
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("unknown"), None);
    }

    #[test]
    fn parses_enabled_only_inside_requested_section() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "[other]\nenabled = true\n[mcp_servers.liveagent_workflow]\nenabled = false\n[next]\nenabled = true\n",
        )
        .expect("write config");
        assert_eq!(
            parse_section_enabled(&path, "mcp_servers.liveagent_workflow").expect("parse section"),
            Some(false)
        );
    }
}
