use std::env;
use std::path::PathBuf;
use std::time::Duration;

use paneforge_adapter::{
    read_paneforge_discovery_file, require_dispatch_requalification, run_read_only_preflight,
    AdapterTimeouts, PaneForgeAdapter,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = PreflightArgs::parse()?;
    let config = read_paneforge_discovery_file(&args.discovery, args.discovery_generation)?;
    let mut adapter = PaneForgeAdapter::new(
        config,
        AdapterTimeouts::new(
            Duration::from_millis(args.connect_timeout_ms),
            Duration::from_millis(args.request_timeout_ms),
        ),
    )?;
    let report = run_read_only_preflight(&mut adapter);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if let Err(error) = require_dispatch_requalification(&report) {
        eprintln!(
            "dispatchRequalificationError={}",
            serde_json::to_string(&error)?
        );
        std::process::exit(2);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreflightArgs {
    discovery: PathBuf,
    discovery_generation: u64,
    connect_timeout_ms: u64,
    request_timeout_ms: u64,
}

impl PreflightArgs {
    fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut discovery = env::var_os("CODEX_PANEFORGE_DISCOVERY_FILE").map(PathBuf::from);
        let mut discovery_generation = 1_u64;
        let mut connect_timeout_ms = 2_000_u64;
        let mut request_timeout_ms = 8_000_u64;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--discovery" => discovery = args.next().map(PathBuf::from),
                "--discovery-generation" => {
                    discovery_generation = parse_u64(args.next(), "--discovery-generation")?;
                }
                "--connect-timeout-ms" => {
                    connect_timeout_ms = parse_u64(args.next(), "--connect-timeout-ms")?;
                }
                "--request-timeout-ms" => {
                    request_timeout_ms = parse_u64(args.next(), "--request-timeout-ms")?;
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other}").into()),
            }
        }
        let Some(discovery) = discovery else {
            print_usage();
            return Err("missing --discovery or CODEX_PANEFORGE_DISCOVERY_FILE".into());
        };
        Ok(Self {
            discovery,
            discovery_generation,
            connect_timeout_ms,
            request_timeout_ms,
        })
    }
}

fn parse_u64(value: Option<String>, name: &'static str) -> Result<u64, Box<dyn std::error::Error>> {
    let Some(value) = value else {
        return Err(format!("{name} requires a value").into());
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|error| format!("{name} must be an integer: {error}"))?;
    if parsed == 0 {
        return Err(format!("{name} must be greater than zero").into());
    }
    Ok(parsed)
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run -p paneforge-adapter --example read_only_preflight -- --discovery <paneforge-mcp-discovery.json>"
    );
}
