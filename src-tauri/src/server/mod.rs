//! Local hook listener. One dedicated blocking thread running a `tiny_http`
//! server — chosen over axum (no need for a full async runtime to handle a
//! few POSTs an hour from one local client) and over a hand-rolled
//! `TcpListener` (HTTP/1.1 parsing correctness isn't worth reinventing).
//!
//! Binds to `127.0.0.1` ONLY, never `0.0.0.0` — this must never be reachable
//! from the network. Fixed port (see `port::DEFAULT_PORT`) — we deliberately
//! do NOT scan for a fallback port on conflict, because the hook URL is
//! already baked into the user's `~/.claude/settings.json` by the time this
//! runs again; silently moving to a different port would silently break it.
//! `tauri-plugin-single-instance` (registered in lib.rs) is what prevents a
//! second launch of Samlu itself from ever hitting that conflict.
//!
//! Routes: `POST /hooks/{adapter}/{event}/{token}` — e.g.
//! `/hooks/claude-code/notification/<TOKEN>`. The `{adapter}` segment lets
//! future adapters (Codex, Cursor, ...) register their own routes without
//! colliding with Claude Code's.

pub mod auth;
pub mod port;

use crate::adapters::AdapterRegistry;
use crate::agents::approvals::{response_body, Decision};
use crate::agents::{origin::SessionOrigin, ApprovalRequest, HookMessage};
use crate::events::AdapterError;
use crate::state::AppState;
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tiny_http::{Method, Request, Response, Server};

const MAX_REQUEST_BODY_BYTES: u64 = 1_048_576;

/// How often a held Claude request gets a whitespace chunk. A failed write
/// is how the listener learns the client gave up (the user answered in the
/// terminal), so this is also the worst-case detection lag per write.
const HEARTBEAT: Duration = Duration::from_secs(2);

/// A held approval, as the hub opened it.
pub struct OpenApproval {
    pub id: String,
    pub decisions: Receiver<Decision>,
    /// How long the listener keeps the request open before "no decision".
    pub hold: Duration,
}

/// What the listener needs from the rest of the app. The hub implements it;
/// tests substitute a fake so the HTTP side can be exercised alone.
pub trait HookHost: Send + Sync + 'static {
    /// Everything that is not an approval.
    fn handle(&self, message: HookMessage);
    /// Registers a held request. An unusable island answers it at once, so
    /// the receiver then already holds `NoDecision`.
    fn begin_approval(&self, request: ApprovalRequest) -> OpenApproval;
    /// The hold ran out with nobody answering.
    fn hold_elapsed(&self, request_id: &str);
    /// The client closed the connection before an answer was written.
    fn client_gone(&self, request_id: &str);
}

struct Context {
    token: String,
    adapters: Arc<AdapterRegistry>,
    host: Arc<dyn HookHost>,
    heartbeat: Duration,
}

/// Starts the listener on a background thread. Returns the bound port on
/// success (always `port::DEFAULT_PORT` — see module doc comment).
pub fn start(
    app: AppHandle,
    state: Arc<AppState>,
    adapters: Arc<AdapterRegistry>,
) -> Result<u16, String> {
    let bind_port = port::DEFAULT_PORT;
    let server = Server::http(("127.0.0.1", bind_port))
        .map_err(|e| format!("failed to bind 127.0.0.1:{bind_port}: {e}"))?;
    let host = Arc::new(crate::agents::hub::HubHost::new(app));
    serve(server, state.token.clone(), adapters, host, HEARTBEAT);
    Ok(bind_port)
}

/// One thread per request: a held approval must never stall the next hook.
fn serve(
    server: Server,
    token: String,
    adapters: Arc<AdapterRegistry>,
    host: Arc<dyn HookHost>,
    heartbeat: Duration,
) {
    let context = Arc::new(Context {
        token,
        adapters,
        host,
        heartbeat,
    });
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let context = context.clone();
            thread::spawn(move || handle_connection(request, &context));
        }
    });
}

/// What a request turned into: an answer now, or an approval to hold.
enum Routed {
    Reply(Response<std::io::Cursor<Vec<u8>>>),
    Hold(Box<ApprovalRequest>),
}

fn handle_connection(mut request: Request, context: &Context) {
    match route(context, &mut request) {
        Routed::Reply(response) => {
            let _ = request.respond(response);
        }
        Routed::Hold(approval) => hold(request, context, *approval),
    }
}

fn reply(status: u16, body: &str) -> Routed {
    Routed::Reply(Response::from_string(body).with_status_code(status))
}

fn route(context: &Context, request: &mut Request) -> Routed {
    if *request.method() != Method::Post {
        return reply(405, "method not allowed");
    }

    let full_url = request.url().to_string();
    let path = full_url.split('?').next().unwrap_or(&full_url);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

    // Expect exactly: hooks / {adapter} / {event} / {token}
    if segments.len() != 4 || segments[0] != "hooks" {
        return reply(404, "not found");
    }
    let adapter_name = segments[1];
    let event_route = segments[2];
    let token = segments[3];

    if !auth::verify(&context.token, token) {
        return reply(401, "unauthorized");
    }

    if request
        .body_length()
        .is_some_and(|length| length as u64 > MAX_REQUEST_BODY_BYTES)
    {
        return reply(413, "payload too large");
    }

    let mut body = Vec::new();
    if request
        .as_reader()
        .take(MAX_REQUEST_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .is_err()
    {
        return reply(400, "bad request");
    }
    if body.len() as u64 > MAX_REQUEST_BODY_BYTES {
        return reply(413, "payload too large");
    }

    let origin = SessionOrigin::from_headers(
        request
            .headers()
            .iter()
            .map(|header| (header.field.as_str().as_str(), header.value.as_str())),
    );

    // A body that does not parse is answered 400 here, so nothing is held:
    // both agents treat a non-2xx answer as "no decision".
    match context
        .adapters
        .parse(adapter_name, event_route, &body, &origin)
    {
        Ok(HookMessage::Approval(approval)) => Routed::Hold(Box::new(approval)),
        Ok(message) => {
            context.host.handle(message);
            reply(204, "")
        }
        Err(AdapterError::Ignored) => reply(204, ""),
        Err(AdapterError::InvalidJson(detail)) => {
            log::warn!(
                "hook payload for {adapter_name}/{event_route} was not valid JSON: {detail}"
            );
            reply(400, "bad request")
        }
        Err(AdapterError::UnknownRoute(detail)) => {
            log::warn!("no adapter/route matched: {detail}");
            reply(404, "not found")
        }
    }
}

/// Keeps `request` open until the user answers or the hold ends. Claude
/// requests stream whitespace so a dropped connection shows up as a failed
/// write; Codex requests are short and get one plain response.
fn hold(request: Request, context: &Context, approval: ApprovalRequest) {
    let opened = context.host.begin_approval(approval.clone());
    if approval.agent == "claude-code" {
        hold_chunked(request, context, &approval, opened);
    } else {
        hold_plain(request, context, &approval, opened);
    }
}

/// The hold ran out. An answer may still have landed in the same instant;
/// the hub has already recorded it, so it wins over "no decision".
fn decision_at_deadline(context: &Context, opened: &OpenApproval) -> Decision {
    context.host.hold_elapsed(&opened.id);
    opened.decisions.try_recv().unwrap_or(Decision::NoDecision)
}

fn hold_plain(
    request: Request,
    context: &Context,
    approval: &ApprovalRequest,
    opened: OpenApproval,
) {
    let decision = match opened.decisions.recv_timeout(opened.hold) {
        Ok(decision) => decision,
        Err(RecvTimeoutError::Timeout) => decision_at_deadline(context, &opened),
        Err(RecvTimeoutError::Disconnected) => Decision::NoDecision,
    };
    let body = response_body(&approval.agent, decision, approval);
    let _ = request.respond(Response::from_data(body).with_status_code(200));
}

fn hold_chunked(
    request: Request,
    context: &Context,
    approval: &ApprovalRequest,
    opened: OpenApproval,
) {
    let mut writer = request.into_writer();
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    if writer.write_all(head).and_then(|_| writer.flush()).is_err() {
        context.host.client_gone(&opened.id);
        return;
    }

    let deadline = Instant::now() + opened.hold;
    let decision = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match opened
            .decisions
            .recv_timeout(remaining.min(context.heartbeat))
        {
            Ok(decision) => break decision,
            Err(RecvTimeoutError::Disconnected) => break Decision::NoDecision,
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    break decision_at_deadline(context, &opened);
                }
                if write_chunk(&mut writer, b" ").is_err() {
                    context.host.client_gone(&opened.id);
                    return;
                }
            }
        }
    };

    let body = response_body(&approval.agent, decision, approval);
    let _ = write_chunk(&mut writer, &body).and_then(|_| {
        writer.write_all(b"0\r\n\r\n")?;
        writer.flush()
    });
}

/// One HTTP/1.1 chunk, flushed so the peer sees it now. Empty data would
/// end the body, so it is skipped.
fn write_chunk(writer: &mut dyn Write, data: &[u8]) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    write!(writer, "{:x}\r\n", data.len())?;
    writer.write_all(data)?;
    writer.write_all(b"\r\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::TcpStream;
    use std::sync::mpsc::{self, SyncSender};
    use std::sync::Mutex;

    const TOKEN: &str = "tok";

    /// Records what the listener asks of the hub and lets the test play the
    /// user: it answers through the sender the "hub" kept.
    struct FakeHost {
        hold: Duration,
        senders: Mutex<HashMap<String, SyncSender<Decision>>>,
        opened: Mutex<mpsc::Sender<String>>,
        handled: Mutex<Vec<String>>,
        elapsed: Mutex<Vec<String>>,
        /// Answers inside `hold_elapsed`, as a click landing at the deadline.
        answer_at_deadline: Mutex<Option<Decision>>,
        gone: Mutex<Vec<String>>,
        counter: Mutex<u32>,
    }

    impl FakeHost {
        fn new(hold: Duration) -> (Arc<Self>, Receiver<String>) {
            let (opened, opened_rx) = mpsc::channel();
            let host = Arc::new(Self {
                hold,
                senders: Mutex::new(HashMap::new()),
                opened: Mutex::new(opened),
                handled: Mutex::new(Vec::new()),
                elapsed: Mutex::new(Vec::new()),
                answer_at_deadline: Mutex::new(None),
                gone: Mutex::new(Vec::new()),
                counter: Mutex::new(0),
            });
            (host, opened_rx)
        }

        fn answer(&self, id: &str, decision: Decision) {
            let sender = self.senders.lock().unwrap().remove(id).unwrap();
            sender.send(decision).unwrap();
        }
    }

    impl HookHost for FakeHost {
        fn handle(&self, message: HookMessage) {
            let label = match message {
                HookMessage::Event(event) => format!("event:{}", event.session_id),
                HookMessage::Signal(signal) => format!("signal:{}", signal.session_id),
                HookMessage::Approval(_) => "approval".to_string(),
            };
            self.handled.lock().unwrap().push(label);
        }

        fn begin_approval(&self, _request: ApprovalRequest) -> OpenApproval {
            let (sender, receiver) = mpsc::sync_channel(1);
            let id = {
                let mut counter = self.counter.lock().unwrap();
                *counter += 1;
                format!("req-{counter}")
            };
            self.senders.lock().unwrap().insert(id.clone(), sender);
            let _ = self.opened.lock().unwrap().send(id.clone());
            OpenApproval {
                id,
                decisions: receiver,
                hold: self.hold,
            }
        }

        fn hold_elapsed(&self, request_id: &str) {
            self.elapsed.lock().unwrap().push(request_id.to_string());
            if let Some(decision) = self.answer_at_deadline.lock().unwrap().take() {
                self.answer(request_id, decision);
            }
        }

        fn client_gone(&self, request_id: &str) {
            self.gone.lock().unwrap().push(request_id.to_string());
        }
    }

    /// A listener on an ephemeral port, 20 ms heartbeats.
    fn listen(host: Arc<FakeHost>) -> u16 {
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        serve(
            server,
            TOKEN.to_string(),
            Arc::new(AdapterRegistry::new()),
            host,
            Duration::from_millis(20),
        );
        port
    }

    fn post(port: u16, path: &str, body: &str) -> TcpStream {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream
    }

    /// Reads to EOF, or to the end of a chunked body.
    fn read_response(mut stream: TcpStream) -> (String, String) {
        let mut raw = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buffer[..n]),
            }
            if raw.ends_with(b"0\r\n\r\n") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        (head.to_string(), body.to_string())
    }

    /// Joins chunk payloads; returns None when the framing is not valid.
    fn dechunk(body: &str) -> Option<String> {
        let mut rest = body;
        let mut out = String::new();
        loop {
            let (size, tail) = rest.split_once("\r\n")?;
            let size = usize::from_str_radix(size, 16).ok()?;
            if size == 0 {
                return Some(out);
            }
            out.push_str(tail.get(..size)?);
            rest = tail.get(size..)?.strip_prefix("\r\n")?;
        }
    }

    fn codex_permission() -> &'static str {
        r#"{"session_id":"t1","cwd":"/p","tool_name":"Bash","tool_input":{"command":"ls"}}"#
    }

    fn claude_permission() -> &'static str {
        r#"{"session_id":"s1","cwd":"/p","tool_name":"Bash","tool_input":{"command":"ls"}}"#
    }

    fn request_for(agent: &str) -> ApprovalRequest {
        ApprovalRequest {
            agent_id: None,
            agent: agent.to_string(),
            session_id: String::new(),
            project_path: String::new(),
            tool_name: String::new(),
            action: String::new(),
            always_allow: None,
            origin: SessionOrigin::default(),
        }
    }

    #[test]
    fn codex_permission_is_held_until_the_hub_answers() {
        let (host, opened) = FakeHost::new(Duration::from_secs(30));
        let port = listen(host.clone());
        let stream = post(
            port,
            &format!("/hooks/codex/permission/{TOKEN}"),
            codex_permission(),
        );
        let id = opened.recv_timeout(Duration::from_secs(5)).unwrap();
        host.answer(&id, Decision::Deny);
        let (head, body) = read_response(stream);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(!head.to_lowercase().contains("chunked"));
        assert_eq!(
            body.as_bytes(),
            response_body("codex", Decision::Deny, &request_for("codex"))
        );
    }

    #[test]
    fn codex_hold_elapsed_answers_with_an_empty_body() {
        let (host, _opened) = FakeHost::new(Duration::from_millis(100));
        let port = listen(host.clone());
        let stream = post(
            port,
            &format!("/hooks/codex/permission/{TOKEN}"),
            codex_permission(),
        );
        let (head, body) = read_response(stream);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, "");
        assert_eq!(host.elapsed.lock().unwrap().as_slice(), ["req-1"]);
    }

    #[test]
    fn claude_permission_streams_heartbeats_then_the_decision() {
        let (host, opened) = FakeHost::new(Duration::from_secs(30));
        let port = listen(host.clone());
        let stream = post(
            port,
            &format!("/hooks/claude-code/permission/{TOKEN}"),
            claude_permission(),
        );
        let id = opened.recv_timeout(Duration::from_secs(5)).unwrap();
        thread::sleep(Duration::from_millis(150));
        host.answer(&id, Decision::Allow);
        let (head, body) = read_response(stream);
        assert!(head.to_lowercase().contains("transfer-encoding: chunked"));
        assert!(body.starts_with("1\r\n \r\n"), "heartbeats first: {body:?}");
        let joined = dechunk(&body).expect("valid chunked framing");
        let want = response_body("claude-code", Decision::Allow, &request_for("claude-code"));
        assert_eq!(joined.trim_start().as_bytes(), want);
    }

    #[test]
    fn an_answer_landing_at_the_deadline_is_not_lost() {
        for agent in ["codex", "claude-code"] {
            let (host, _opened) = FakeHost::new(Duration::from_millis(100));
            *host.answer_at_deadline.lock().unwrap() = Some(Decision::Allow);
            let port = listen(host.clone());
            let payload = if agent == "codex" {
                codex_permission()
            } else {
                claude_permission()
            };
            let stream = post(port, &format!("/hooks/{agent}/permission/{TOKEN}"), payload);
            let (_, body) = read_response(stream);
            let body = if agent == "codex" {
                body
            } else {
                dechunk(&body).unwrap().trim_start().to_string()
            };
            assert_eq!(
                body.as_bytes(),
                response_body(agent, Decision::Allow, &request_for(agent)),
                "{agent}"
            );
        }
    }

    #[test]
    fn claude_hold_elapsed_ends_with_empty_json() {
        let (host, _opened) = FakeHost::new(Duration::from_millis(100));
        let port = listen(host.clone());
        let stream = post(
            port,
            &format!("/hooks/claude-code/permission/{TOKEN}"),
            claude_permission(),
        );
        let (_, body) = read_response(stream);
        assert_eq!(dechunk(&body).unwrap().trim(), "{}");
        assert_eq!(host.elapsed.lock().unwrap().as_slice(), ["req-1"]);
    }

    #[test]
    fn claude_dropped_connection_reports_client_gone() {
        let (host, opened) = FakeHost::new(Duration::from_secs(30));
        let port = listen(host.clone());
        let mut stream = post(
            port,
            &format!("/hooks/claude-code/permission/{TOKEN}"),
            claude_permission(),
        );
        opened.recv_timeout(Duration::from_secs(5)).unwrap();
        // Wait for the headers so the listener is in its heartbeat loop.
        let mut first = [0u8; 16];
        stream.read_exact(&mut first).unwrap();
        drop(stream);
        let deadline = Instant::now() + Duration::from_secs(5);
        while host.gone.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(host.gone.lock().unwrap().as_slice(), ["req-1"]);
    }

    #[test]
    fn unparseable_permission_is_400_and_never_held() {
        let (host, _opened) = FakeHost::new(Duration::from_secs(30));
        let port = listen(host.clone());
        for agent in ["codex", "claude-code"] {
            let stream = post(
                port,
                &format!("/hooks/{agent}/permission/{TOKEN}"),
                "not json",
            );
            let (head, _) = read_response(stream);
            assert!(head.starts_with("HTTP/1.1 400"), "{agent}: {head}");
        }
        assert!(host.senders.lock().unwrap().is_empty());
    }

    #[test]
    fn wrong_token_is_401_and_signals_reach_the_host() {
        let (host, _opened) = FakeHost::new(Duration::from_secs(30));
        let port = listen(host.clone());
        let (head, _) = read_response(post(port, "/hooks/codex/prompt/nope", codex_permission()));
        assert!(head.starts_with("HTTP/1.1 401"), "{head}");
        let (head, _) = read_response(post(
            port,
            &format!("/hooks/codex/prompt/{TOKEN}"),
            codex_permission(),
        ));
        assert!(head.starts_with("HTTP/1.1 204"), "{head}");
        assert_eq!(host.handled.lock().unwrap().as_slice(), ["signal:t1"]);
    }
}
