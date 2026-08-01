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
use crate::events::AdapterError;
use crate::notify;
use crate::state::AppState;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::thread;
use tauri::AppHandle;
use tiny_http::{Method, Request, Response, Server};

const MAX_REQUEST_BODY_BYTES: u64 = 1_048_576;

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

    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let response = handle_request(&app, &state, &adapters, &mut request);
            let _ = request.respond(response);
        }
    });

    Ok(bind_port)
}

fn handle_request(
    app: &AppHandle,
    state: &Arc<AppState>,
    adapters: &Arc<AdapterRegistry>,
    request: &mut Request,
) -> Response<Cursor<Vec<u8>>> {
    if *request.method() != Method::Post {
        return Response::from_string("method not allowed").with_status_code(405);
    }

    let full_url = request.url().to_string();
    let path = full_url.split('?').next().unwrap_or(&full_url);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

    // Expect exactly: hooks / {adapter} / {event} / {token}
    if segments.len() != 4 || segments[0] != "hooks" {
        return Response::from_string("not found").with_status_code(404);
    }
    let adapter_name = segments[1];
    let event_route = segments[2];
    let token = segments[3];

    if !auth::verify(&state.token, token) {
        return Response::from_string("unauthorized").with_status_code(401);
    }

    if request
        .body_length()
        .is_some_and(|length| length as u64 > MAX_REQUEST_BODY_BYTES)
    {
        return Response::from_string("payload too large").with_status_code(413);
    }

    let mut body = Vec::new();
    if request
        .as_reader()
        .take(MAX_REQUEST_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .is_err()
    {
        return Response::from_string("bad request").with_status_code(400);
    }
    if body.len() as u64 > MAX_REQUEST_BODY_BYTES {
        return Response::from_string("payload too large").with_status_code(413);
    }

    match adapters.parse(adapter_name, event_route, &body) {
        Ok(event) => {
            notify::handle_event(app.clone(), state.clone(), event);
            Response::from_string("").with_status_code(204)
        }
        Err(AdapterError::Ignored) => Response::from_string("").with_status_code(204),
        Err(AdapterError::InvalidJson(detail)) => {
            log::warn!(
                "hook payload for {adapter_name}/{event_route} was not valid JSON: {detail}"
            );
            Response::from_string("bad request").with_status_code(400)
        }
        Err(AdapterError::UnknownRoute(detail)) => {
            log::warn!("no adapter/route matched: {detail}");
            Response::from_string("not found").with_status_code(404)
        }
    }
}
