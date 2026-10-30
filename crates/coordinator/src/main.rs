use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use coordination_coordinator::{CoordinatorConfig, CoordinatorRuntime};
use coordination_core::{CoordinationError, ErrorCode, RequestId};
use coordination_protocol::{IpcEnvelope, IpcResponseEnvelope, IPC_SCHEMA_VERSION};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut database_path = None::<PathBuf>;
    let mut data_dir = None::<PathBuf>;
    let mut ipc_endpoint = String::from("codex-paneforge-coordinator-v1");
    let mut stdio_ipc = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("codex-paneforge-coordinator {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--stdio-ipc" => stdio_ipc = true,
            "--database" => database_path = args.next().map(PathBuf::from),
            "--data-dir" => data_dir = args.next().map(PathBuf::from),
            "--ipc-endpoint" => {
                if let Some(value) = args.next() {
                    ipc_endpoint = value;
                }
            }
            _ => {}
        }
    }
    let data_dir =
        data_dir.unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let database_path = database_path.unwrap_or_else(|| data_dir.join("coordination.db"));
    let runtime = CoordinatorRuntime::open(
        &database_path,
        &data_dir,
        &ipc_endpoint,
        CoordinatorConfig::default(),
    )?;
    if stdio_ipc {
        run_stdio_ipc(runtime)?;
        return Ok(());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&runtime.health_response()?)?
    );
    Ok(())
}

fn run_stdio_ipc(mut runtime: CoordinatorRuntime) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let response = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(value) => match IpcEnvelope::from_value(value) {
                Ok(envelope) => runtime.handle_ipc(envelope),
                Err(error) => ipc_error("invalid-ipc", error),
            },
            Err(error) => ipc_error(
                "parse-error",
                CoordinationError::new(
                    ErrorCode::InvalidRequest,
                    format!("parse IPC JSON line failed: {error}"),
                ),
            ),
        };
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}

fn ipc_error(request_id: &str, error: CoordinationError) -> IpcResponseEnvelope {
    IpcResponseEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new(request_id)
            .unwrap_or_else(|_| RequestId::try_new("ipc-error").unwrap()),
        ok: false,
        response: None,
        error: Some(error),
    }
}
