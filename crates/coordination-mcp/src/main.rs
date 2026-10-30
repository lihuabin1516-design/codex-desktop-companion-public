use std::io;

use coordination_mcp::{
    default_caller_id, serve, CoordinatorProcessConfig, McpServer, ProcessCoordinatorClient,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    if let Some(argument) = args.next() {
        match argument.as_str() {
            "--version" | "-V" => {
                println!("codex-ccpanes-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--describe" => {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "kind": "coordination-mcp",
                        "name": "codex-ccpanes-mcp",
                        "version": env!("CARGO_PKG_VERSION"),
                        "mcpSchemaVersion": coordination_protocol::MCP_SCHEMA_VERSION,
                        "ipcSchemaVersion": coordination_protocol::IPC_SCHEMA_VERSION,
                        "target": format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
                    }))?
                );
                return Ok(());
            }
            _ => {
                return Err(format!("unsupported argument: {argument}").into());
            }
        }
    }

    let config = CoordinatorProcessConfig::from_env()?;
    let client = ProcessCoordinatorClient::connect_or_start(config)?;
    let mut server = McpServer::new(client, default_caller_id());
    serve(&mut server, io::stdin().lock(), io::stdout())?;
    Ok(())
}
