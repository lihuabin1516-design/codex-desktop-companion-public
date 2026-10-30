use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use codex_paneforge::audit::{audit_builds, BuildAuditReport};
use codex_paneforge::config::{migrate, MigrationReport, MigrationRequest};
use codex_paneforge::diagnostics::{doctor, probe, DoctorOptions, DoctorReport, ProbeReport};
use codex_paneforge::launcher::{launch, LaunchRequest};
use codex_paneforge::{descriptor, PRODUCT_NAME};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "codex-paneforge", version, about = PRODUCT_NAME)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Safely launch the installed official PaneForge.
    Start(StartArgs),
    /// Run production health checks without changing Codex or PaneForge data.
    Doctor(DoctorArgs),
    /// Probe the installed official PaneForge executable.
    Probe(ProbeArgs),
    /// Read-only scan for Rust target directories.
    AuditBuilds(AuditArgs),
    /// Preview or apply migration into the Companion configuration schema.
    MigrateConfig(MigrateArgs),
    /// Print the stable product descriptor.
    Describe(OutputArgs),
}

#[derive(Debug, Args)]
struct StartArgs {
    #[arg(long)]
    paneforge_bin: Option<PathBuf>,
    #[arg(long)]
    wait: bool,
    #[arg(long)]
    json: bool,
    #[arg(last = true, allow_hyphen_values = true)]
    arguments: Vec<OsString>,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long)]
    paneforge_bin: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    show_paths: bool,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ProbeArgs {
    #[arg(long)]
    paneforge_bin: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    show_paths: bool,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct AuditArgs {
    root: PathBuf,
    #[arg(long, default_value_t = 5.0)]
    threshold_gb: f64,
    #[arg(long, default_value_t = 8)]
    max_depth: usize,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    show_paths: bool,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    #[arg(long)]
    paneforge_bin: Option<PathBuf>,
    #[arg(long = "from")]
    legacy_config: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    show_paths: bool,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct OutputArgs {
    #[arg(long)]
    json: bool,
    #[arg(long)]
    output: Option<PathBuf>,
}

fn main() {
    match execute() {
        Ok(code) => process::exit(code),
        Err(error) => {
            eprintln!("codex-paneforge error: {error:#}");
            process::exit(2);
        }
    }
}

fn execute() -> Result<i32> {
    let cli = Cli::parse();
    match cli.command.unwrap_or_else(|| {
        Commands::Start(StartArgs {
            paneforge_bin: None,
            wait: false,
            json: false,
            arguments: Vec::new(),
        })
    }) {
        Commands::Start(args) => {
            let receipt = launch(LaunchRequest {
                explicit_target: args.paneforge_bin,
                arguments: args.arguments,
                wait: args.wait,
            })?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "started official PaneForge pid={} source={} sanitized={}",
                    receipt.pid,
                    receipt.target_source,
                    receipt.removed_environment_keys.len()
                );
            }
            Ok(receipt.exit_code.unwrap_or(0))
        }
        Commands::Doctor(args) => {
            let report = doctor(DoctorOptions {
                explicit_target: args.paneforge_bin,
                show_paths: args.show_paths,
            });
            emit_report(
                &report,
                args.json,
                args.output.as_deref(),
                human_doctor(&report),
            )?;
            Ok(report.exit_code())
        }
        Commands::Probe(args) => {
            let report = probe(args.paneforge_bin.as_deref(), args.show_paths)?;
            emit_report(
                &report,
                args.json,
                args.output.as_deref(),
                human_probe(&report),
            )?;
            Ok(0)
        }
        Commands::AuditBuilds(args) => {
            if !args.threshold_gb.is_finite() || args.threshold_gb < 0.0 {
                anyhow::bail!("threshold_gb_must_be_non_negative");
            }
            let threshold_bytes = (args.threshold_gb * 1024.0 * 1024.0 * 1024.0) as u64;
            let report =
                audit_builds(&args.root, threshold_bytes, args.max_depth, args.show_paths)?;
            let has_large = report
                .findings
                .iter()
                .any(|finding| finding.exceeds_threshold);
            emit_report(
                &report,
                args.json,
                args.output.as_deref(),
                human_audit(&report),
            )?;
            Ok(i32::from(has_large))
        }
        Commands::MigrateConfig(args) => {
            let report = migrate(MigrationRequest {
                explicit_target: args.paneforge_bin,
                legacy_config: args.legacy_config,
                target_config: args.config,
                apply: args.apply,
                show_paths: args.show_paths,
            })?;
            emit_report(
                &report,
                args.json,
                args.output.as_deref(),
                human_migration(&report),
            )?;
            Ok(0)
        }
        Commands::Describe(args) => {
            let report = descriptor();
            emit_report(
                &report,
                args.json,
                args.output.as_deref(),
                format!(
                    "{} {} ({})",
                    report.name, report.version, report.upstream_policy
                ),
            )?;
            Ok(0)
        }
    }
}

fn emit_report<T: Serialize>(
    report: &T,
    json_output: bool,
    output_path: Option<&Path>,
    human: String,
) -> Result<()> {
    let json = serde_json::to_string_pretty(report)?;
    if let Some(path) = output_path {
        atomic_write(path, json.as_bytes())?;
    }
    if json_output {
        println!("{json}");
    } else {
        println!("{human}");
        if let Some(path) = output_path {
            println!("report={}", path.display());
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("create report directory {}", parent.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output_path_has_no_filename"))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}", process::id()));
    fs::write(&temporary, bytes)
        .with_context(|| format!("write temporary report {}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("replace existing report {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("publish report {}", path.display()))?;
    Ok(())
}

fn human_doctor(report: &DoctorReport) -> String {
    let mut lines = vec![format!("Codex + PaneForge doctor: {:?}", report.overall)];
    for item in &report.checks {
        lines.push(format!("- {:?} {}: {}", item.status, item.id, item.message));
    }
    lines.join("\n")
}

fn human_probe(report: &ProbeReport) -> String {
    format!(
        "PaneForge available={} source={} version={} bytes={} sha256={}",
        report.available,
        report.source,
        report.product_version.as_deref().unwrap_or("unknown"),
        report.bytes,
        report.sha256
    )
}

fn human_audit(report: &BuildAuditReport) -> String {
    let large = report
        .findings
        .iter()
        .filter(|finding| finding.exceeds_threshold)
        .count();
    let mut lines = vec![format!(
        "Rust target audit: scanned={} above-threshold={} read-errors={}",
        report.scanned_target_directories, large, report.read_errors
    )];
    for finding in &report.findings {
        let location = finding.path.as_deref().unwrap_or(&finding.path_hash);
        lines.push(format!(
            "- {:.2} GiB {} {}",
            finding.bytes as f64 / 1024.0 / 1024.0 / 1024.0,
            if finding.exceeds_threshold {
                "WARN"
            } else {
                "OK"
            },
            location
        ));
    }
    lines.join("\n")
}

fn human_migration(report: &MigrationReport) -> String {
    format!(
        "Config migration: applied={} source={} before={} after={} candidate={}",
        report.applied,
        report.source,
        if report.before.exists {
            "present"
        } else {
            "missing"
        },
        if report.after.exists {
            "present"
        } else {
            "missing"
        },
        report
            .candidate_path
            .as_deref()
            .unwrap_or(&report.candidate_path_hash)
    )
}
