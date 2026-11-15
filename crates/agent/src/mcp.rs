//! A Model Context Protocol server over stdin/stdout (DESIGN §6.27).
//!
//! Serves revision 2026-07-28 statelessly (every request carries its version
//! in `_meta`) and the `initialize` handshake of 2025-11-25 and 2025-06-18
//! for older clients. One JSON message per line; stdout carries protocol
//! messages and nothing else; the server exits when stdin closes.
//!
//! The main thread only reads, and one thread only writes. Each
//! `tools/call` runs on its own worker with a cancel token that
//! `notifications/cancelled` sets; the writer drops a cancelled call's
//! messages before writing them, as the stdio transport requires.

use crate::devices::Failure;
use crate::tools::{self, Report};
use gust_core::lister::CancelToken;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
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

/// Progress is dropped, never queued, once this many messages wait.
const PROGRESS_BACKLOG: usize = 64;

/// stdout, written by one thread of its own: a client that stops reading
/// stdout can stall that thread, never the one reading stdin, so requests,
/// cancellations and stdin closing are always seen.
#[derive(Clone)]
struct Out {
    tx: mpsc::Sender<Queued>,
    pending: Arc<AtomicUsize>,
}

/// One line for stdout, the call it belongs to, and what to do once it has
/// been written or dropped.
struct Queued {
    call: Option<CancelToken>,
    line: String,
    then: Option<Box<dyn FnOnce() + Send>>,
}

impl Out {
    fn start(mut output: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = mpsc::channel::<Queued>();
        let pending = Arc::new(AtomicUsize::new(0));
        let left = pending.clone();
        let _ = std::thread::Builder::new()
            .name("gust-mcp-out".into())
            .spawn(move || {
                for q in rx {
                    left.fetch_sub(1, Ordering::AcqRel);
                    // A cancelled call's messages are dropped here, before a
                    // byte of them is written. One already being written
                    // when the cancellation arrives was already sent.
                    if !q.call.as_ref().is_some_and(CancelToken::is_cancelled) {
                        // A closed stdout means the client is gone; stdin's
                        // EOF ends the server.
                        let _ = output
                            .write_all(q.line.as_bytes())
                            .and_then(|_| output.flush());
                    }
                    if let Some(then) = q.then {
                        then();
                    }
                }
            });
        Out { tx, pending }
    }

    /// Queue one message; `call` is the request it belongs to, if any.
    fn send(&self, message: &Value, call: Option<&CancelToken>) {
        if call.is_some_and(CancelToken::is_cancelled) {
            return;
        }
        self.queue(message, call, None);
    }

    /// Queue a call's last message; `then` runs once it is written or
    /// dropped, so the call stays cancellable while its answer waits.
    fn send_last(&self, message: &Value, call: &CancelToken, then: Box<dyn FnOnce() + Send>) {
        self.queue(message, Some(call), Some(then));
    }

    fn queue(
        &self,
        message: &Value,
        call: Option<&CancelToken>,
        then: Option<Box<dyn FnOnce() + Send>>,
    ) {
        let mut line = message.to_string();
        line.push('\n');
        self.pending.fetch_add(1, Ordering::AcqRel);
        let queued = Queued {
            call: call.cloned(),
            line,
            then,
        };
        if let Err(mpsc::SendError(q)) = self.tx.send(queued) {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            if let Some(then) = q.then {
                then();
            }
        }
    }

    fn backlogged(&self) -> bool {
        self.pending.load(Ordering::Acquire) >= PROGRESS_BACKLOG
    }
}

fn write(out: &Out, message: &Value) {
    out.send(message, None);
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

/// A call: its number (so a finished one removes only itself), its cancel
/// token, and whether its tool is still working (false once only its answer
/// is left to write).
type Running = (u64, CancelToken, Arc<AtomicBool>);

struct Server {
    out: Out,
    /// The revision `initialize` agreed on, for clients of that era.
    legacy: Option<&'static str>,
    /// Calls by request id, until their answer is written or dropped, so a
    /// cancellation can still reach an answer waiting for stdout. Only calls
    /// still working and not cancelled count against the limits: a stalled
    /// device can keep a cancelled worker blocked in the kernel (§6.4.3),
    /// but it will never write again.
    in_flight: Arc<Mutex<HashMap<String, Running>>>,
    /// The send that is running, if any: one at a time.
    sending: Arc<Mutex<Option<(u64, CancelToken)>>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Serve stdin/stdout until stdin closes. The exit status for `gust --mcp`.
pub fn run() -> i32 {
    serve(std::io::stdin().lock(), Box::new(std::io::stdout()))
}

pub fn serve(mut input: impl BufRead, output: Box<dyn Write + Send>) -> i32 {
    let mut server = Server {
        out: Out::start(output),
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
            // Drop the rest of the oversized line, a buffer at a time.
            if !discard_line(&mut input) {
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
    for (_, token, _) in lock(&server.in_flight).values() {
        token.cancel();
    }
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while server.workers.iter().any(|w| !w.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    0
}

/// Skip input up to and including the next newline without keeping it.
/// False when stdin ended or failed first.
fn discard_line(input: &mut impl BufRead) -> bool {
    loop {
        let (used, found) = match input.fill_buf() {
            Ok([]) | Err(_) => return false,
            Ok(buf) => match buf.iter().position(|b| *b == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            },
        };
        input.consume(used);
        if found {
            return true;
        }
    }
}

fn key(id: &Value) -> String {
    id.to_string()
}

impl Server {
    fn message(&mut self, m: Map<String, Value>) {
        if m.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            // Answer a malformed request; a malformed notification gets nothing.
            if let Some(id) = m.get("id") {
                let id = if id.is_string() || id.is_i64() || id.is_u64() {
                    id.clone()
                } else {
                    Value::Null
                };
                write(
                    &self.out,
                    &error(&id, INVALID_REQUEST, "jsonrpc must be \"2.0\"", None),
                );
            }
            return;
        }
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
            let token = params.get("requestId").and_then(|id| {
                lock(&self.in_flight)
                    .get(&key(id))
                    .map(|(_, t, _)| t.clone())
            });
            if let Some(token) = token {
                token.cancel();
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
            // Gone from 2026-07-28; kept for clients of the handshake era.
            "ping" => Some(match self.era(id, &params) {
                Ok(era) => ok(id, finish(era, json!({}), false)),
                Err(e) => e,
            }),
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
        let working = Arc::new(AtomicBool::new(true));
        let number = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
        let is_send = name == "send_files";
        {
            let mut in_flight = lock(&self.in_flight);
            if in_flight
                .get(&key(id))
                .is_some_and(|(_, t, _)| !t.is_cancelled())
            {
                return Some(error(
                    id,
                    INVALID_REQUEST,
                    "A request with this id is still running",
                    None,
                ));
            }
            if in_flight
                .values()
                .filter(|(_, t, working)| working.load(Ordering::Acquire) && !t.is_cancelled())
                .count()
                >= MAX_IN_FLIGHT
            {
                return busy("Too many requests are running.");
            }
            if is_send {
                let mut sending = lock(&self.sending);
                if sending.as_ref().is_some_and(|(_, t)| !t.is_cancelled()) {
                    return busy("Another send is still running.");
                }
                *sending = Some((number, cancel.clone()));
            }
            in_flight.insert(key(id), (number, cancel.clone(), working.clone()));
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
                working.store(false, Ordering::Release);
                release_send(&sending, number);
                out.send_last(
                    &reply,
                    &cancel,
                    Box::new(move || forget(&in_flight, &reply_id, number)),
                );
            });
        match worker {
            Ok(w) => {
                self.workers.push(w);
                None
            }
            Err(_) => {
                release_send(&self.sending, number);
                forget(&self.in_flight, id, number);
                Some(error(id, INTERNAL_ERROR, "Could not start the tool", None))
            }
        }
    }
}

/// Free the send slot once a send's work is done, unless a newer send
/// already holds it.
fn release_send(sending: &Mutex<Option<(u64, CancelToken)>>, number: u64) {
    let mut sending = lock(sending);
    if sending.as_ref().is_some_and(|(n, _)| *n == number) {
        *sending = None;
    }
}

/// Forget a call whose answer was written or dropped, unless its id already
/// belongs to a newer call.
fn forget(in_flight: &Mutex<HashMap<String, Running>>, id: &Value, number: u64) {
    let mut in_flight = lock(in_flight);
    if in_flight
        .get(&key(id))
        .is_some_and(|(n, _, _)| *n == number)
    {
        in_flight.remove(&key(id));
    }
}

fn tool_result(r: Result<Value, Failure>) -> Value {
    match r {
        Ok(v) => json!({
            "content": [{"type": "text", "text": v.to_string()}],
            "structuredContent": v,
            "isError": false,
        }),
        Err(f) => {
            let mut e = json!({"code": f.code, "message": f.message, "action": f.action});
            if let Some(d) = f.details {
                e["details"] = d;
            }
            json!({
                "content": [{"type": "text", "text": json!({"error": e}).to_string()}],
                "isError": true,
            })
        }
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
        if self.cancel.is_cancelled() || self.out.backlogged() {
            return;
        }
        *last = Some(progress);
        let note = json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params});
        self.out.send(&note, Some(&self.cancel));
    }
}
