use std::io::{BufRead, Write};

use crate::ipc_client::CoordinatorIpcClient;
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::server::McpServer;

pub fn serve<R, W, C>(
    server: &mut McpServer<C>,
    reader: R,
    mut writer: W,
) -> Result<(), Box<dyn std::error::Error>>
where
    R: BufRead,
    W: Write,
    C: CoordinatorIpcClient,
{
    for line in reader.lines() {
        let line = line?;
        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => server.handle(request),
            Err(error) => Some(JsonRpcResponse::error(
                serde_json::Value::Null,
                -32700,
                format!("parse error: {error}"),
                None,
            )),
        };
        if let Some(response) = response {
            writeln!(writer, "{}", serde_json::to_string(&response)?)?;
            writer.flush()?;
        }
    }
    Ok(())
}
