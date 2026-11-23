//! Streamable-HTTP MCP adapter (ADR-118).
//!
//! PdfCraft MCP over a dedicated thread (the engine session is not `Send`).
//! Each `(X-MCP-Actor, workspace root)` pair gets one Headless session
//! confined to that workspace. Missing or invalid roots make write tools
//! fail; they never fall back to the process data dir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use pdfcraft_automation::file_access::AutomationRoots;
use pdfcraft_automation::mcp::McpServer;
use pdfcraft_automation::Automation;
use serde_json::{Value, json};
use tokio::sync::oneshot;

const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

#[derive(Clone, Eq, PartialEq, Hash)]
struct SessionKey {
    actor: String,
    workspace: String,
}

struct Job {
    actor: String,
    workspace_root: Option<String>,
    msg: Value,
    reply: oneshot::Sender<Option<Value>>,
}

struct Session {
    server: McpServer,
    last_doc: Option<PathBuf>,
}

#[derive(Clone)]
pub struct McpHub {
    tx: Sender<Job>,
}

static HUB: OnceLock<McpHub> = OnceLock::new();

impl McpHub {
    pub fn spawn() -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("pdfcraft-mcp".into())
            .spawn(move || worker(rx))
            .map_err(|e| format!("failed to start pdfcraft-mcp thread: {e}"))?;
        Ok(Self { tx })
    }

    pub fn install(self) {
        let _ = HUB.set(self);
    }

    fn global() -> Option<Self> {
        HUB.get().cloned()
    }

    pub async fn handle(&self, actor: String, workspace_root: Option<String>, msg: Value) -> Option<Value> {
        let (reply, rx) = oneshot::channel();
        if self
            .tx
            .send(Job {
                actor,
                workspace_root,
                msg,
                reply,
            })
            .is_err()
        {
            return Some(rpc_error(Value::Null, INTERNAL_ERROR, "pdfcraft-mcp thread is gone"));
        }
        match rx.await {
            Ok(v) => v,
            Err(_) => Some(rpc_error(Value::Null, INTERNAL_ERROR, "pdfcraft-mcp worker dropped the reply")),
        }
    }
}

fn worker(rx: Receiver<Job>) {
    let mut sessions: HashMap<SessionKey, Session> = HashMap::new();
    while let Ok(job) = rx.recv() {
        let out = dispatch(&mut sessions, &job.actor, job.workspace_root.as_deref(), job.msg);
        let _ = job.reply.send(out);
    }
}

fn dispatch(
    sessions: &mut HashMap<SessionKey, Session>,
    actor: &str,
    workspace_root: Option<&str>,
    mut msg: Value,
) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned().unwrap_or(Value::Null);

    if method == "tools/call" {
        let root = match require_workspace_dir(workspace_root) {
            Ok(p) => p,
            Err(m) => return Some(rpc_error(id, INVALID_PARAMS, &m)),
        };
        let tool = msg
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        rebase_tool_paths(&mut msg, &root);
        
        let path_arg = msg
            .get("params")
            .and_then(|p| p.get("arguments"))
            .and_then(|a| a.get("path"))
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let key = SessionKey {
            actor: actor.to_string(),
            workspace: root.to_string_lossy().into_owned(),
        };
        let session = match sessions.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => match open_confined_server(&root) {
                Ok(s) => v.insert(Session { server: s, last_doc: None }),
                Err(m) => return Some(rpc_error(id, INVALID_PARAMS, &m)),
            },
        };
        let out = session.server.handle(&msg);
        if let Some(ref result) = out
            && !tool_failed(result)
        {
            remember_doc(&mut session.last_doc, &tool, path_arg.as_deref());
            persist_preview_png(&session.last_doc, &tool, result);
        }
        return out;
    }

    let key = SessionKey {
        actor: actor.to_string(),
        workspace: workspace_root
            .and_then(|s| canonical_dir(s).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let session = sessions.entry(key).or_insert_with(|| Session {
        server: workspace_root
            .and_then(|s| canonical_dir(s).ok())
            .and_then(|p| open_confined_server(&p).ok())
            .unwrap_or_else(|| default_server()),
        last_doc: None,
    });
    session.server.handle(&msg)
}

fn tool_failed(out: &Value) -> bool {
    out.get("error").is_some()
        || out.get("result").and_then(|r| r.get("isError")).and_then(Value::as_bool) == Some(true)
}

fn remember_doc(last_doc: &mut Option<PathBuf>, tool: &str, path: Option<&Path>) {
    if tool != "doc_open" && tool != "doc_save" {
        return;
    }
    let Some(path) = path else { return };
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        *last_doc = Some(path.to_path_buf());
    }
}

fn persist_preview_png(last_doc: &Option<PathBuf>, tool: &str, out: &Value) {
    let shot = tool == "screenshot"
        || tool == "render_slide"
        || tool == "page_render"
        || tool == "render_preview"
        || tool == "render_page";
    if !shot {
        return;
    }
    let Some(doc) = last_doc.as_ref() else { return };
    let Some(png_path) = preview_png_sibling(doc) else { return };
    let Some(bytes) = png_from_mcp_result(out) else { return };
    if let Err(e) = std::fs::write(&png_path, bytes) {
        log::warn!("pdfcraft preview png {}: {e}", png_path.display());
    }
}

pub fn preview_png_sibling(doc: &Path) -> Option<PathBuf> {
    let stem = doc.file_stem()?.to_str()?;
    Some(doc.with_file_name(format!("{stem}.preview.png")))
}

pub fn png_from_mcp_result(out: &Value) -> Option<Vec<u8>> {
    let content = out
        .get("result")
        .and_then(|r| r.get("content"))
        .or_else(|| out.get("content"))
        .and_then(Value::as_array)?;
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("image") {
            continue;
        }
        let mime = block.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
        if mime != "image/png" {
            continue;
        }
        let data = block.get("data").and_then(Value::as_str)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
        if bytes.starts_with(b"\x89PNG") {
            return Some(bytes);
        }
    }
    None
}

fn open_confined_server(root: &Path) -> Result<McpServer, String> {
    let roots = AutomationRoots::new(Some(root), Some(root))?;
    let auto = Automation::new()
        .with_root(root)
        .map_err(|e| e.to_string())?
        .with_automation_roots(roots);
    Ok(McpServer::new(auto))
}
fn default_server() -> McpServer {
    McpServer::new(Automation::new())
}



fn require_workspace_dir(raw: Option<&str>) -> Result<PathBuf, String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(
            "X-MCP-Workspace-Root is required; file tools are confined to the Agent workspace"
                .into(),
        );
    };
    canonical_dir(raw)
}

fn canonical_dir(raw: &str) -> Result<PathBuf, String> {
    match std::fs::canonicalize(raw) {
        Ok(p) if p.is_dir() => Ok(p),
        Ok(_) => Err(format!("X-MCP-Workspace-Root is not a directory: {raw}")),
        Err(e) => Err(format!("X-MCP-Workspace-Root {raw}: {e}")),
    }
}

pub fn rebase_tool_paths(msg: &mut Value, root: &Path) {
    let Some(args) = msg.get_mut("params").and_then(|p| p.get_mut("arguments")) else {
        return;
    };
    rebase_object(args, root);
}

fn rebase_object(v: &mut Value, root: &Path) {
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    for key in ["path", "dir"] {
        if let Some(s) = obj.get(key).and_then(Value::as_str) {
            let next = rebase_one(s, root);
            obj.insert(key.to_string(), json!(next));
        }
    }
    if let Some(s) = obj.get("variables").and_then(Value::as_str) {
        let next = rebase_one(s, root);
        obj.insert("variables".into(), json!(next));
    }
    if let Some(params) = obj.get_mut("params") {
        rebase_object(params, root);
    }
    if let Some(cmds) = obj.get_mut("commands").and_then(Value::as_array_mut) {
        for cmd in cmds {
            rebase_object(cmd, root);
        }
    }
}

fn rebase_one(path: &str, root: &Path) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        path.to_string()
    } else {
        root.join(p).to_string_lossy().into_owned()
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub async fn mcp_handler(headers: HeaderMap, body: Bytes) -> Response {
    let Some(hub) = McpHub::global() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"error": "pdfcraft-mcp not ready"})),
        )
            .into_response();
    };
    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let actor = header_str(&headers, "x-mcp-actor").unwrap_or_default();
    let workspace = header_str(&headers, "x-mcp-workspace-root");
    match hub.handle(actor, workspace, req).await {
        None => StatusCode::ACCEPTED.into_response(),
        Some(v) => (StatusCode::OK, axum::Json(v)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_join_the_workspace() {
        let root = Path::new("/tmp/ws");
        let mut msg = json!({
            "params": {"arguments": {"path": "docs/a.bin"}}
        });
        rebase_tool_paths(&mut msg, root);
        let got = msg["params"]["arguments"]["path"].as_str().unwrap();
        assert!(got.contains("docs"));
        assert!(got.contains("ws"));
    }
}
