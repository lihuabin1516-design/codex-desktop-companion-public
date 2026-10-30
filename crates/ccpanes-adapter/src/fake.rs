use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::capability::{
    default_fake_tools, TOOL_ADD_PROJECT_TO_WORKSPACE, TOOL_CREATE_WORKSPACE,
    TOOL_GET_SESSION_OUTPUT, TOOL_GET_SESSION_STATUS, TOOL_KILL_SESSION, TOOL_LAUNCH_TASK,
    TOOL_LIST_PANES, TOOL_LIST_PROJECTS, TOOL_LIST_SESSIONS, TOOL_LIST_WORKSPACES,
    TOOL_SUBMIT_TO_SESSION, TOOL_WAIT_FOR_SESSION, TOOL_WRITE_TO_SESSION,
};
use crate::CCPANES_MCP_PROTOCOL_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeResponseMode {
    Json,
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeJsonRpcError {
    pub code: i64,
    pub message: String,
}

impl FakeJsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSessionSeed {
    pub task_id: String,
    pub session_id: String,
    pub binding_id: String,
    pub status: String,
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub client_request_id: Option<String>,
    pub created_at_ms: i64,
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct FakeCcpanesBehavior {
    pub required_authorization: Option<String>,
    pub response_mode: FakeResponseMode,
    pub hang_after_request: bool,
    pub drop_launch_response_after_side_effect: bool,
    pub complete_launches: bool,
    pub tools: Vec<Value>,
    pub workspaces: Vec<Value>,
    pub projects: Vec<Value>,
    pub sessions: Vec<FakeSessionSeed>,
    pub tool_text_errors: BTreeMap<String, String>,
    pub jsonrpc_errors: BTreeMap<String, FakeJsonRpcError>,
}

impl Default for FakeCcpanesBehavior {
    fn default() -> Self {
        Self {
            required_authorization: None,
            response_mode: FakeResponseMode::Json,
            hang_after_request: false,
            drop_launch_response_after_side_effect: false,
            complete_launches: false,
            tools: default_fake_tools(),
            workspaces: Vec::new(),
            projects: Vec::new(),
            sessions: Vec::new(),
            tool_text_errors: BTreeMap::new(),
            jsonrpc_errors: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FakeRequestRecord {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone)]
struct FakeSession {
    task_id: String,
    session_id: String,
    binding_id: String,
    status: String,
    title: Option<String>,
    prompt: Option<String>,
    client_request_id: Option<String>,
    created_at_ms: i64,
    output: String,
}

impl From<FakeSessionSeed> for FakeSession {
    fn from(seed: FakeSessionSeed) -> Self {
        Self {
            task_id: seed.task_id,
            session_id: seed.session_id,
            binding_id: seed.binding_id,
            status: seed.status,
            title: seed.title,
            prompt: seed.prompt,
            client_request_id: seed.client_request_id,
            created_at_ms: seed.created_at_ms,
            output: seed.output,
        }
    }
}

#[derive(Debug, Default)]
struct FakeState {
    requests: Vec<FakeRequestRecord>,
    sessions: Vec<FakeSession>,
    workspaces: Vec<Value>,
    projects: Vec<Value>,
    next_session: u64,
    submitted: Vec<(String, String)>,
}

pub struct FakeCcpanesServer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<FakeState>>,
    handle: Option<JoinHandle<()>>,
}

impl FakeCcpanesServer {
    pub fn start(behavior: FakeCcpanesBehavior) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(false)?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let sessions = behavior
            .sessions
            .clone()
            .into_iter()
            .map(FakeSession::from)
            .collect::<Vec<_>>();
        let next_session = sessions.len() as u64;
        let state = Arc::new(Mutex::new(FakeState {
            workspaces: behavior.workspaces.clone(),
            projects: behavior.projects.clone(),
            sessions,
            next_session,
            ..FakeState::default()
        }));
        let server_stop = Arc::clone(&stop);
        let server_state = Arc::clone(&state);
        let handle = thread::spawn(move || {
            while !server_stop.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    continue;
                };
                if server_stop.load(Ordering::SeqCst) {
                    break;
                }
                handle_connection(stream, &behavior, &server_state);
            }
        });
        Ok(Self {
            endpoint,
            stop,
            state,
            handle: Some(handle),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn records(&self) -> Vec<FakeRequestRecord> {
        self.state.lock().expect("fake state lock").requests.clone()
    }

    pub fn last_tool_arguments(&self, tool_name: &str) -> Option<Value> {
        self.records()
            .into_iter()
            .rev()
            .find(|record| record.tool_name.as_deref() == Some(tool_name))
            .and_then(|record| record.arguments)
    }
}

impl Drop for FakeCcpanesServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(
            self.endpoint
                .trim_start_matches("http://")
                .trim_end_matches("/mcp"),
        );
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    behavior: &FakeCcpanesBehavior,
    state: &Arc<Mutex<FakeState>>,
) {
    let Ok((headers, body)) = read_http_request(&mut stream) else {
        return;
    };
    if behavior.hang_after_request {
        thread::sleep(Duration::from_millis(750));
        return;
    }
    let request: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let tool_name = request
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let arguments = request
        .get("params")
        .and_then(|params| params.get("arguments"))
        .cloned();
    let authorization = header_value(&headers, "authorization");
    let authorization_digest = authorization.map(digest_value);
    {
        let mut state = state.lock().expect("fake state lock");
        state.requests.push(FakeRequestRecord {
            method: method.clone(),
            tool_name: tool_name.clone(),
            authorization_digest,
            arguments: arguments.clone(),
        });
    }

    if let Some(required) = &behavior.required_authorization {
        if authorization != Some(required.as_str()) {
            write_jsonrpc_error(
                stream,
                &id,
                401,
                -32001,
                "authorization failed",
                behavior.response_mode,
            );
            return;
        }
    }

    let error_key = tool_name
        .as_ref()
        .map(|tool| format!("{method}:{tool}"))
        .unwrap_or_else(|| method.clone());
    if let Some(error) = behavior
        .jsonrpc_errors
        .get(&error_key)
        .or_else(|| behavior.jsonrpc_errors.get(&method))
    {
        write_jsonrpc_error(
            stream,
            &id,
            200,
            error.code,
            &error.message,
            behavior.response_mode,
        );
        return;
    }

    if method == "tools/call"
        && tool_name.as_deref() == Some(TOOL_LAUNCH_TASK)
        && behavior.drop_launch_response_after_side_effect
    {
        let _ = fake_launch(
            arguments.unwrap_or_else(|| json!({})),
            behavior.complete_launches,
            state,
        );
        return;
    }

    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": CCPANES_MCP_PROTOCOL_VERSION,
            "serverInfo": {"name": "fake-cc-panes", "version": "0.0.0-fake"}
        }),
        "notifications/initialized" => json!({}),
        "tools/list" => json!({"tools": behavior.tools}),
        "tools/call" => handle_tool_call(tool_name.as_deref(), arguments, behavior, state),
        _ => json!({
            "content": [{"type": "text", "text": format!("error: method {method} not found")}],
            "isError": true
        }),
    };
    write_jsonrpc_result(stream, &id, result, behavior.response_mode);
}

fn handle_tool_call(
    tool_name: Option<&str>,
    arguments: Option<Value>,
    behavior: &FakeCcpanesBehavior,
    state: &Arc<Mutex<FakeState>>,
) -> Value {
    let tool_name = tool_name.unwrap_or_default();
    if let Some(message) = behavior.tool_text_errors.get(tool_name) {
        return json!({
            "content": [{"type": "text", "text": message}],
            "isError": false
        });
    }
    match tool_name {
        TOOL_LAUNCH_TASK => fake_launch(
            arguments.unwrap_or_else(|| json!({})),
            behavior.complete_launches,
            state,
        ),
        TOOL_LIST_SESSIONS => fake_list_sessions(state),
        TOOL_LIST_PANES => fake_list_panes(state),
        TOOL_LIST_WORKSPACES => fake_list_workspaces(state),
        TOOL_LIST_PROJECTS => fake_list_projects(state),
        TOOL_CREATE_WORKSPACE => fake_create_workspace(arguments.as_ref(), state),
        TOOL_ADD_PROJECT_TO_WORKSPACE => fake_add_project(arguments.as_ref(), state),
        TOOL_GET_SESSION_STATUS => fake_session_status(arguments.as_ref(), state),
        TOOL_GET_SESSION_OUTPUT => fake_session_output(arguments.as_ref(), state),
        TOOL_SUBMIT_TO_SESSION => fake_submit(arguments.as_ref(), state),
        TOOL_KILL_SESSION => fake_kill(arguments.as_ref(), state),
        TOOL_WAIT_FOR_SESSION => fake_wait(arguments.as_ref(), state),
        TOOL_WRITE_TO_SESSION => fake_write(arguments.as_ref(), state),
        _ => json!({
            "content": [{"type": "text", "text": format!("error: tool {tool_name} not found")}],
            "isError": true
        }),
    }
}

fn fake_launch(arguments: Value, complete_launches: bool, state: &Arc<Mutex<FakeState>>) -> Value {
    let mut state = state.lock().expect("fake state lock");
    state.next_session = state.next_session.saturating_add(1);
    let number = state.next_session;
    let project_path = arguments
        .get("projectPath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prompt = arguments
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let output = if prompt
        .as_deref()
        .is_some_and(|prompt| prompt.contains("CCPANES_CANARY_CWD"))
    {
        format!("synthetic output for session-{number}\nCCPANES_CANARY_CWD={project_path}")
    } else {
        format!("synthetic output for session-{number}")
    };
    let session = FakeSession {
        task_id: format!("task-{number}"),
        session_id: format!("session-{number}"),
        binding_id: format!("binding-{number}"),
        status: if complete_launches {
            "completed".to_owned()
        } else {
            "running".to_owned()
        },
        title: arguments
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
        prompt,
        client_request_id: arguments
            .get("clientRequestId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        created_at_ms: arguments
            .get("createdAtMs")
            .and_then(Value::as_i64)
            .unwrap_or(1_000 + number as i64),
        output,
    };
    let payload = json!({
        "taskId": session.task_id,
        "sessionId": session.session_id,
        "bindingId": session.binding_id,
        "runtimeKind": arguments
            .get("runtimeKind")
            .and_then(Value::as_str)
            .unwrap_or("local")
    });
    state.sessions.push(session);
    tool_payload(payload)
}

fn fake_list_sessions(state: &Arc<Mutex<FakeState>>) -> Value {
    let sessions = state
        .lock()
        .expect("fake state lock")
        .sessions
        .iter()
        .map(|session| {
            json!({
                "taskId": session.task_id,
                "sessionId": session.session_id,
                "bindingId": session.binding_id,
                "status": session.status,
                "createdAtMs": session.created_at_ms,
                "metadata": {"clientRequestId": session.client_request_id}
            })
        })
        .collect::<Vec<_>>();
    tool_payload(json!({"sessions": sessions}))
}

fn fake_list_panes(state: &Arc<Mutex<FakeState>>) -> Value {
    let tabs = state
        .lock()
        .expect("fake state lock")
        .sessions
        .iter()
        .map(|session| {
            json!({
                "sessionId": session.session_id,
                "title": session.title,
                "prompt": session.prompt,
                "createdAtMs": session.created_at_ms
            })
        })
        .collect::<Vec<_>>();
    tool_payload(json!({
        "panes": [{"tabs": tabs.clone()}],
        "layouts": [{"panes": [{"tabs": tabs}]}]
    }))
}

fn fake_list_workspaces(state: &Arc<Mutex<FakeState>>) -> Value {
    let workspaces = state.lock().expect("fake state lock").workspaces.clone();
    tool_payload(json!({ "workspaces": workspaces }))
}

fn fake_list_projects(state: &Arc<Mutex<FakeState>>) -> Value {
    let projects = state.lock().expect("fake state lock").projects.clone();
    tool_payload(json!({ "projects": projects }))
}

fn fake_create_workspace(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let Some(name) = arguments
        .and_then(|arguments| arguments.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
    else {
        return text_error("error: name is required");
    };
    let path = arguments
        .and_then(|arguments| arguments.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty());
    let mut state = state.lock().expect("fake state lock");
    let id = format!("workspace-{}", state.workspaces.len() + 1);
    let workspace = json!({ "id": id, "name": name, "path": path });
    state.workspaces.push(workspace.clone());
    tool_payload(workspace)
}

fn fake_add_project(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let Some(workspace_name) = arguments
        .and_then(|arguments| arguments.get("workspaceName"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
    else {
        return text_error("error: workspaceName is required");
    };
    let Some(project_path) = arguments
        .and_then(|arguments| arguments.get("projectPath"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
    else {
        return text_error("error: projectPath is required");
    };
    let mut state = state.lock().expect("fake state lock");
    if state
        .workspaces
        .iter()
        .all(|workspace| workspace.get("name").and_then(Value::as_str) != Some(workspace_name))
    {
        return text_error("error: workspaceName was not found");
    }
    let id = format!("project-{}", state.projects.len() + 1);
    let project_name = project_name_from_path(project_path);
    let project = json!({
        "id": id,
        "name": project_name,
        "path": project_path,
        "workspaceName": workspace_name
    });
    state.projects.push(project.clone());
    tool_payload(project)
}

fn project_name_from_path(project_path: &str) -> String {
    project_path
        .trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("synthetic-project")
        .to_owned()
}

fn fake_session_status(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let Some(session_id) = arguments
        .and_then(|arguments| arguments.get("sessionId"))
        .and_then(Value::as_str)
    else {
        return text_error("error: sessionId is required");
    };
    let state = state.lock().expect("fake state lock");
    let Some(session) = state
        .sessions
        .iter()
        .find(|session| session.session_id == session_id)
    else {
        return session_not_found(session_id);
    };
    tool_payload(json!({"sessionId": session.session_id, "status": session.status}))
}

fn fake_session_output(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let Some(session_id) = arguments
        .and_then(|arguments| arguments.get("sessionId"))
        .and_then(Value::as_str)
    else {
        return text_error("error: sessionId is required");
    };
    let state = state.lock().expect("fake state lock");
    let Some(session) = state
        .sessions
        .iter()
        .find(|session| session.session_id == session_id)
    else {
        return session_not_found(session_id);
    };
    tool_payload(json!({"sessionId": session.session_id, "output": session.output}))
}

fn fake_submit(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let session_id = arguments
        .and_then(|arguments| arguments.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = arguments
        .and_then(|arguments| arguments.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    state
        .lock()
        .expect("fake state lock")
        .submitted
        .push((session_id.to_owned(), text.to_owned()));
    tool_payload(json!({"accepted": true}))
}

fn fake_kill(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let Some(session_id) = arguments
        .and_then(|arguments| arguments.get("sessionId"))
        .and_then(Value::as_str)
    else {
        return text_error("error: sessionId is required");
    };
    let mut state = state.lock().expect("fake state lock");
    let Some(session) = state
        .sessions
        .iter_mut()
        .find(|session| session.session_id == session_id)
    else {
        return session_not_found(session_id);
    };
    session.status = "cancelled".to_owned();
    tool_payload(json!({"accepted": true, "sessionId": session_id}))
}

fn fake_wait(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let Some(session_id) = arguments
        .and_then(|arguments| arguments.get("sessionId"))
        .and_then(Value::as_str)
    else {
        return text_error("error: sessionId is required");
    };
    let wait_for = arguments
        .and_then(|arguments| arguments.get("waitFor"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = state.lock().expect("fake state lock");
    let Some(session) = state
        .sessions
        .iter()
        .find(|session| session.session_id == session_id)
    else {
        return session_not_found(session_id);
    };
    let satisfied = wait_for
        .iter()
        .filter_map(Value::as_str)
        .any(|status| status.eq_ignore_ascii_case(&session.status));
    tool_payload(json!({
        "satisfied": satisfied,
        "finalStatus": session.status,
        "waitedMs": 0,
        "turnSeq": 0
    }))
}

fn fake_write(arguments: Option<&Value>, state: &Arc<Mutex<FakeState>>) -> Value {
    let session_id = arguments
        .and_then(|arguments| arguments.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = arguments
        .and_then(|arguments| arguments.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    state
        .lock()
        .expect("fake state lock")
        .submitted
        .push((session_id.to_owned(), text.to_owned()));
    tool_payload(json!({"success": true, "sessionId": session_id}))
}

fn tool_payload(payload: Value) -> Value {
    json!({
        "structuredContent": payload,
        "content": [{"type": "text", "text": serde_json::to_string(&payload).unwrap_or_default()}],
        "isError": false
    })
}

fn text_error(message: impl AsRef<str>) -> Value {
    json!({
        "content": [{"type": "text", "text": message.as_ref()}],
        "isError": true
    })
}

fn session_not_found(session_id: &str) -> Value {
    text_error(format!(
        "error: [NOT_FOUND] Session not found: {session_id}"
    ))
}

fn read_http_request(
    stream: &mut TcpStream,
) -> std::io::Result<(BTreeMap<String, String>, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if header_end(&bytes).is_some() {
            break;
        }
    }
    let Some(header_end) = header_end(&bytes) else {
        return Ok((BTreeMap::new(), Vec::new()));
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]).to_string();
    let mut headers = BTreeMap::new();
    for line in head.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let content_length = header_value(&headers, "content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    let mut body = bytes.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(content_length);
    Ok((headers, body))
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers.get(&name.to_ascii_lowercase()).map(String::as_str)
}

fn write_jsonrpc_result(mut stream: TcpStream, id: &Value, result: Value, mode: FakeResponseMode) {
    let payload = json!({"jsonrpc": "2.0", "id": id, "result": result});
    write_http_payload(&mut stream, 200, &payload, mode);
}

fn write_jsonrpc_error(
    mut stream: TcpStream,
    id: &Value,
    status: u16,
    code: i64,
    message: &str,
    mode: FakeResponseMode,
) {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    });
    write_http_payload(&mut stream, status, &payload, mode);
}

fn write_http_payload(
    stream: &mut TcpStream,
    status: u16,
    payload: &Value,
    mode: FakeResponseMode,
) {
    let json = serde_json::to_string(payload).unwrap_or_default();
    let (content_type, body) = match mode {
        FakeResponseMode::Json => ("application/json", json),
        FakeResponseMode::Sse => ("text/event-stream", format!("data: {json}\n\n")),
    };
    let reason = if status == 200 { "OK" } else { "ERROR" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nMcp-Session-Id: fake-mcp-session\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn digest_value(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
