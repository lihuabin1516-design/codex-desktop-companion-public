use std::env;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use coordination_core::{CoordinationError, ErrorCode, RequestId};
use coordination_protocol::{
    require_schema_version, IpcEnvelope, IpcHello, IpcMessage, IpcResponse, IpcResponseEnvelope,
    IPC_SCHEMA_VERSION,
};

use crate::MCP_SERVER_NAME;

pub trait CoordinatorIpcClient {
    fn exchange(&mut self, envelope: IpcEnvelope) -> Result<IpcResponse, CoordinationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorProcessConfig {
    pub coordinator_exe: PathBuf,
    pub database_path: PathBuf,
    pub data_dir: PathBuf,
    pub ipc_endpoint: String,
}

impl CoordinatorProcessConfig {
    pub fn from_env() -> Result<Self, CoordinationError> {
        let data_dir = env::var_os("CODEX_CCPANES_COORDINATION_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        let database_path = env::var_os("CODEX_CCPANES_COORDINATION_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("coordination.db"));
        let coordinator_exe = env::var_os("CODEX_CCPANES_COORDINATOR_EXE")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(default_coordinator_exe)?;
        let ipc_endpoint = env::var("CODEX_CCPANES_COORDINATION_IPC_ENDPOINT")
            .unwrap_or_else(|_| "codex-ccpanes-coordinator-v1".to_owned());
        Ok(Self {
            coordinator_exe,
            database_path,
            data_dir,
            ipc_endpoint,
        })
    }
}

pub struct ProcessCoordinatorClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ProcessCoordinatorClient {
    pub fn connect_or_start(config: CoordinatorProcessConfig) -> Result<Self, CoordinationError> {
        let mut child = Command::new(&config.coordinator_exe)
            .arg("--stdio-ipc")
            .arg("--database")
            .arg(&config.database_path)
            .arg("--data-dir")
            .arg(&config.data_dir)
            .arg("--ipc-endpoint")
            .arg(&config.ipc_endpoint)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                CoordinationError::new(
                    ErrorCode::AdapterNotReady,
                    format!("start coordinator IPC process failed: {error}"),
                )
                .with_retry_after_ms(1_000)
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "coordinator IPC child stdin was not captured",
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "coordinator IPC child stdout was not captured",
            )
        })?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        client.handshake()?;
        Ok(client)
    }

    fn handshake(&mut self) -> Result<(), CoordinationError> {
        let envelope = IpcEnvelope {
            schema_version: IPC_SCHEMA_VERSION.to_owned(),
            request_id: RequestId::try_new("mcp-hello")?,
            message: IpcMessage::Hello(IpcHello {
                client_name: MCP_SERVER_NAME.to_owned(),
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
                protocol_version: IPC_SCHEMA_VERSION.to_owned(),
                nonce: "stdio-ipc".to_owned(),
            }),
        };
        match self.exchange(envelope)? {
            IpcResponse::Hello { protocol_version } => {
                require_schema_version(&protocol_version, IPC_SCHEMA_VERSION)?;
                Ok(())
            }
            _ => Err(CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "coordinator IPC hello returned an unexpected response",
            )),
        }
    }
}

impl CoordinatorIpcClient for ProcessCoordinatorClient {
    fn exchange(&mut self, envelope: IpcEnvelope) -> Result<IpcResponse, CoordinationError> {
        let expected_request_id = envelope.request_id.clone();
        let encoded = serde_json::to_string(&envelope).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                format!("encode IPC envelope failed: {error}"),
            )
        })?;
        writeln!(self.stdin, "{encoded}").map_err(map_ipc_io)?;
        self.stdin.flush().map_err(map_ipc_io)?;

        let mut line = String::new();
        let bytes = self.stdout.read_line(&mut line).map_err(map_ipc_io)?;
        if bytes == 0 {
            return Err(CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "coordinator IPC closed before response",
            )
            .with_retry_after_ms(1_000));
        }
        let response: IpcResponseEnvelope =
            serde_json::from_str(line.trim_end()).map_err(|error| {
                CoordinationError::new(
                    ErrorCode::InvalidRequest,
                    format!("decode IPC response failed: {error}"),
                )
            })?;
        require_schema_version(&response.schema_version, IPC_SCHEMA_VERSION)?;
        if response.request_id != expected_request_id {
            return Err(CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "coordinator IPC response correlation mismatch",
            ));
        }
        if !response.ok {
            return Err(response.error.unwrap_or_else(|| {
                CoordinationError::new(
                    ErrorCode::InternalInvariantViolation,
                    "coordinator IPC error response did not include error payload",
                )
            }));
        }
        response.response.ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "coordinator IPC success response did not include payload",
            )
        })
    }
}

impl Drop for ProcessCoordinatorClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn map_ipc_io(error: std::io::Error) -> CoordinationError {
    CoordinationError::new(
        ErrorCode::AdapterNotReady,
        format!("coordinator IPC transport failed: {error}"),
    )
    .with_retry_after_ms(1_000)
}

fn default_data_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("CodexCCPanes")
        .join("coordination")
}

fn default_coordinator_exe() -> Result<PathBuf, CoordinationError> {
    let current_exe = env::current_exe().map_err(|error| {
        CoordinationError::new(
            ErrorCode::AdapterNotReady,
            format!("resolve current MCP executable failed: {error}"),
        )
    })?;
    let binary = if cfg!(windows) {
        "codex-ccpanes-coordinator.exe"
    } else {
        "codex-ccpanes-coordinator"
    };
    Ok(current_exe
        .parent()
        .map(|parent| parent.join(binary))
        .unwrap_or_else(|| PathBuf::from(binary)))
}
