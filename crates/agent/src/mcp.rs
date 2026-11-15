//! A Model Context Protocol server over stdin/stdout (DESIGN §6.27).
//!
//! Serves revision 2026-07-28 statelessly (every request carries its version
//! in `_meta`) and the `initialize` handshake of 2025-11-25 and 2025-06-18
//! for older clients. One JSON message per line; stdout carries protocol
//! messages and nothing else; the server exits when stdin closes.
//!
//! The main thread only reads. Each `tools/call` runs on its own worker with
//! a cancel token that `notifications/cancelled` sets; a cancelled call sends
//! nothing further, as the stdio transport requires.

use crate::devices::Failure;
use crate::tools::{self, Report};
use gust_core::lister::CancelToken;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MODERN: &str = "2026-07-28";
/// Handshake revisions, newest first.
pub const LEGACY: &[&str] = &["2025-11-25", "2025-06-18"];

const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// One message may not be larger than this; a longer line is refused whole.
const LINE_LIMIT: usize = 4 * 1024 * 1024;
/// Tool calls running at once, and how long a closing server waits for them.
const MAX_IN_FLIGHT: usize = 4;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const UNSUPPORTED_VERSION: i64 = -32022;

fn server_info() -> Value {
    json!({"name": "gust", "title": "Gust", "version": env!("CARGO_PKG_VERSION")})
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Era {
    Modern,
    Legacy,
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn write(out: &Out, message: &Value) {
    let mut line = message.to_string();
    line.push('\n');
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    // A closed stdout means the client is gone; stdin's EOF ends the loop.
    let _ = out.write_all(line.as_bytes()).and_then(|_| out.flush());
}

fn error(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = json!({"code": code, "message": message});
    if let Some(d) = data {
        e["data"] = d;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": e})
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The shape each era expects around a result.
fn finish(era: Era, mut result: Value, cacheable: bool) -> Value {
    if era == Era::Modern {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({ SERVER_INFO_KEY: server_info() });
        if cacheable {
            result["ttlMs"] = json!(0);
            result["cacheScope"] = json!("public");
        }
    }
    result
}

struct Server {
    out: Out,
    /// The revision `initialize` agreed on, for clients of that era.
    legacy: Option<&'static str>,
    in_flight: Arc<Mutex<HashMap<String, CancelToken>>>,
    sending: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

/// Serve stdin/stdout until stdin closes. The exit status for `gust --mcp`.
pub fn run() -> i32 {
    serve(std::io::stdin().lock(), Box::new(std::io::stdout()))
}

pub fn serve(mut input: impl BufRead, output: Box<dyn Write + Send>) -> i32 {
    let mut server = Server {
        out: Arc::new(Mutex::new(output)),
        legacy: None,
        in_flight: Arc::default(),
        sending: Arc::default(),
        workers: Vec::new(),
    };
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut input)
            .take(LINE_LIMIT as u64 + 1)
            .read_until(b'\n', &mut line);
        match read {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > LINE_LIMIT && line.last() != Some(&b'\n') {
            // Drop the rest of the oversized line before reading on.
            let mut rest = Vec::new();
            if input.read_until(b'\n', &mut rest).is_err() {
                break;
            }
            write(
                &server.out,
                &error(&Value::Null, INVALID_REQUEST, "Message too large", None),
            );
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => server.message(m),
            Ok(_) => write(
                &server.out,
                &error(
                    &Value::Null,
                    INVALID_REQUEST,
                    "Expected one JSON-RPC object per line",
                    None,
                ),
            ),
            Err(_) => write(
                &server.out,
                &error(&Value::Null, PARSE_ERROR, "Parse error", None),
            ),
        }
        server.workers.retain(|w| !w.is_finished());
    }
    // The client is gone: stop everything, give workers a moment to stop.
    for token in server
        .in_flight
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
    {
        token.cancel();
    }
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while server.workers.iter().any(|w| !w.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    0
}

fn key(id: &Value) -> String {
    id.to_string()
}

impl Server {
    fn message(&mut self, m: Map<String, Value>) {
        let Some(method) = m.get("method").and_then(Value::as_str) else {
            // A response to a request; this server sends none.
            if !m.contains_key("result") && !m.contains_key("error") {
                let id = m.get("id").cloned().unwrap_or(Value::Null);
                write(
                    &self.out,
                    &error(&id, INVALID_REQUEST, "Missing method", None),
                );
            }
            return;
        };
        let params = match m.get("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(p)) => p.clone(),
            Some(_) => {
                if let Some(id) = m.get("id") {
                    write(
                        &self.out,
                        &error(id, INVALID_PARAMS, "params must be an object", None),
                    );
                }
                return;
            }
        };
        let Some(id) = m.get("id") else {
            self.notification(method, &params);
            return;
        };
        if !(id.is_string() || id.is_i64() || id.is_u64()) {
            write(
                &self.out,
                &error(
                    &Value::Null,
                    INVALID_REQUEST,
                    "id must be a string or an integer",
                    None,
                ),
            );
            return;
        }
        if let Some(reply) = self.request(id, method, params) {
            write(&self.out, &reply);
        }
    }

    fn notification(&mut self, method: &str, params: &Map<String, Value>) {
        if method == "notifications/cancelled" {
            if let Some(id) = params.get("requestId") {
                if let Some(t) = self
                    .in_flight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&key(id))
                {
                    t.cancel();
                }
            }
        }
        // notifications/initialized and anything unknown need no answer.
    }

    /// Which era a request belongs to, from its own `_meta` first.
    fn era(&self, id: &Value, params: &Map<String, Value>) -> Result<Era, Value> {
        let meta = params.get("_meta").and_then(Value::as_object);
        match meta.and_then(|m| m.get(VERSION_KEY)) {
            Some(v) => {
                if v.as_str() != Some(MODERN) {
                    let mut supported = vec![MODERN];
                    supported.extend_from_slice(LEGACY);
                    return Err(error(
                        id,
                        UNSUPPORTED_VERSION,
                        "Unsupported protocol version",
                        Some(json!({"supported": supported, "requested": v})),
                    ));
                }
                if !meta
                    .and_then(|m| m.get(CAPABILITIES_KEY))
                    .is_some_and(Value::is_object)
                {
                    return Err(error(
                        id,
                        INVALID_PARAMS,
                        &format!("Missing _meta {CAPABILITIES_KEY}"),
                        None,
                    ));
                }
                Ok(Era::Modern)
            }
            None if self.legacy.is_some() => Ok(Era::Legacy),
            None => Err(error(
                id,
                INVALID_PARAMS,
                &format!("Missing _meta {VERSION_KEY}"),
                None,
            )),
        }
    }

    fn request(&mut self, id: &Value, method: &str, params: Map<String, Value>) -> Option<Value> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let agreed = LEGACY
                    .iter()
                    .copied()
                    .find(|v| Some(*v) == asked)
                    .unwrap_or(LEGACY[0]);
                self.legacy = Some(agreed);
                Some(ok(
                    id,
                    json!({
                        "protocolVersion": agreed,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": server_info(),
                        "instructions": tools::INSTRUCTIONS,
                    }),
                ))
            }
            "ping" => Some(ok(id, json!({}))),
            "server/discover" => Some(match self.era(id, &params) {
                Ok(Era::Modern) => {
                    let mut supported = vec![MODERN];
                    supported.extend_from_slice(LEGACY);
                    ok(
                        id,
                        finish(
                            Era::Modern,
                            json!({
                                "supportedVersions": supported,
                                "capabilities": {"tools": {}},
                                "instructions": tools::INSTRUCTIONS,
                            }),
                            true,
                        ),
                    )
                }
                Ok(Era::Legacy) => error(id, METHOD_NOT_FOUND, "Method not found", None),
                Err(e) => e,
            }),
            "tools/list" => Some(match self.era(id, &params) {
                Ok(era) => ok(
                    id,
                    finish(era, json!({"tools": tools::definitions()}), true),
                ),
                Err(e) => e,
            }),
            "tools/call" => self.call(id, params),
            _ => Some(error(id, METHOD_NOT_FOUND, "Method not found", None)),
        }
    }

    fn call(&mut self, id: &Value, params: Map<String, Value>) -> Option<Value> {
        let era = match self.era(id, &params) {
            Ok(e) => e,
            Err(e) => return Some(e),
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Some(error(id, INVALID_PARAMS, "Missing tool name", None));
        };
        if !tools::exists(name) {
            return Some(error(
                id,
                INVALID_PARAMS,
                &format!("Unknown tool: {name}"),
                None,
            ));
        }
        let args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(a)) => a.clone(),
            Some(_) => {
                return Some(error(
                    id,
                    INVALID_PARAMS,
                    "arguments must be an object",
                    None,
                ))
            }
        };
        let progress_token = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .filter(|t| t.is_string() || t.is_i64() || t.is_u64())
            .cloned();

        let busy = |message: &str| {
            Some(ok(
                id,
                finish(
                    era,
                    tool_result(Err(Failure::new(
                        "busy",
                        message,
                        "Wait for it to finish, then try again.",
                    ))),
                    false,
                ),
            ))
        };
        let cancel = CancelToken::new();
        {
            let mut in_flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
            if in_flight.contains_key(&key(id)) {
                return Some(error(
                    id,
                    INVALID_REQUEST,
                    "A request with this id is still running",
                    None,
                ));
            }
            if in_flight.len() >= MAX_IN_FLIGHT {
                return busy("Too many requests are running.");
            }
            in_flight.insert(key(id), cancel.clone());
        }
        let is_send = name == "send_files";
        if is_send && self.sending.swap(true, Ordering::AcqRel) {
            self.in_flight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key(id));
            return busy("Another send is still running.");
        }

        let out = self.out.clone();
        let in_flight = self.in_flight.clone();
        let sending = self.sending.clone();
        let reply_id = id.clone();
        let name = name.to_string();
        let worker = std::thread::Builder::new()
            .name("gust-mcp-call".into())
            .spawn(move || {
                let report = Progress {
                    out: out.clone(),
                    token: progress_token,
                    cancel: cancel.clone(),
                    last: Mutex::new(None),
                };
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tools::call(
                        &name,
                        &args,
                        &tools::Call {
                            cancel: &cancel,
                            report: &report,
                        },
                    )
                }));
                let reply = match run {
                    Ok(r) => ok(&reply_id, finish(era, tool_result(r), false)),
                    Err(_) => error(&reply_id, INTERNAL_ERROR, "Internal error", None),
                };
                {
                    // Checked under the writer lock: once a cancellation is
                    // seen, nothing more is written for this request.
                    let mut w = out.lock().unwrap_or_else(|e| e.into_inner());
                    if !cancel.is_cancelled() {
                        let mut line = reply.to_string();
                        line.push('\n');
                        let _ = w.write_all(line.as_bytes()).and_then(|_| w.flush());
                    }
                }
                in_flight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key(&reply_id));
                if is_send {
                    sending.store(false, Ordering::Release);
                }
            });
        match worker {
            Ok(w) => {
                self.workers.push(w);
                None
            }
            Err(_) => {
                self.in_flight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key(id));
                if is_send {
                    self.sending.store(false, Ordering::Release);
                }
                Some(error(id, INTERNAL_ERROR, "Could not start the tool", None))
            }
        }
    }
}

fn tool_result(r: Result<Value, Failure>) -> Value {
    match r {
        Ok(v) => json!({
            "content": [{"type": "text", "text": v.to_string()}],
            "structuredContent": v,
            "isError": false,
        }),
        Err(f) => json!({
            "content": [{"type": "text", "text": json!({"error": {"code": f.code, "message": f.message, "action": f.action}}).to_string()}],
            "isError": true,
        }),
    }
}

struct Progress {
    out: Out,
    token: Option<Value>,
    cancel: CancelToken,
    last: Mutex<Option<f64>>,
}

impl Report for Progress {
    fn progress(&self, progress: f64, total: Option<f64>, message: &str) {
        let Some(token) = &self.token else { return };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|l| progress <= l) || !progress.is_finite() {
            return;
        }
        let mut params = json!({"progressToken": token, "progress": progress, "message": message});
        if let Some(t) = total.filter(|t| *t >= progress) {
            params["total"] = json!(t);
        }
        let note = json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params});
        let mut line = note.to_string();
        line.push('\n');
        let mut w = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if !self.cancel.is_cancelled() {
            *last = Some(progress);
            let _ = w.write_all(line.as_bytes()).and_then(|_| w.flush());
        }
    }
}
