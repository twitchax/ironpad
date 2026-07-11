//! Integration test: full host → server → guest WebSocket relay.
//!
//! Spins up the Axum WS routes on a random port and exercises the complete
//! session lifecycle through real WebSocket connections.

use std::path::PathBuf;
use std::time::Duration;

use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use ironpad_common::protocol::{
    self, ClientId, ControlMessage, ErrorCode, Event, EventEnvelope, MessageKind, Mutation,
    NewCell, Permissions, Response,
};
use ironpad_common::types::CellType;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::ws;
use leptos::config::LeptosOptions;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite;

// ── Helpers ─────────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(5);

/// Build a minimal `AppState` (no Leptos SSR routes needed) around a given
/// `WsState`, so individual tests can tune caps/timeouts.
fn state_with_ws(ws: WsState) -> AppState {
    AppState {
        leptos_options: LeptosOptions::builder().output_name("ironpad-test").build(),
        config: AppConfig {
            data_dir: PathBuf::from("/tmp"),
            cache_dir: PathBuf::from("/tmp"),
            port: 0,
            ironpad_cell_path: PathBuf::from("/tmp"),
            compilation_proxy: None,
        },
        ws,
    }
}

/// Build a minimal `AppState` with a default `WsState`.
fn test_state() -> AppState {
    state_with_ws(WsState::default())
}

/// Build an Axum router with only the WS routes (no Leptos/SSR).
fn ws_router(state: AppState) -> Router {
    Router::new()
        .route("/ws/host", get(ws::ws_host_handler))
        .route("/ws/connect", get(ws::ws_connect_handler))
        .with_state(state)
}

/// Start a test server on a random port, returning the base URL.
async fn start_server(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to random port");
    let addr = listener.local_addr().expect("local addr");
    let app = ws_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("server");
    });
    format!("ws://127.0.0.1:{}", addr.port())
}

/// Serialize a protocol message to JSON.
fn to_json(id: &str, kind: MessageKind) -> String {
    serde_json::to_string(&protocol::Message {
        id: id.to_string(),
        kind,
    })
    .unwrap()
}

/// Read one text frame from a WebSocket stream with a timeout.
async fn recv_text<S>(stream: &mut S) -> String
where
    S: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    timeout(TIMEOUT, async {
        loop {
            match stream.next().await {
                Some(Ok(tungstenite::Message::Text(text))) => return text.to_string(),
                Some(Ok(_)) => {}
                Some(Err(e)) => panic!("ws recv error: {e}"),
                None => panic!("ws stream ended unexpectedly"),
            }
        }
    })
    .await
    .expect("recv timed out")
}

/// Parse a protocol message from JSON.
fn parse_msg(json: &str) -> protocol::Message {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("bad JSON: {e}\n{json}"))
}

/// Send the mandatory `ClaimHost` first frame on a freshly-connected host socket
/// (PRD-0038 T-014).
async fn send_claim<S>(sink: &mut S, secret: &str)
where
    S: SinkExt<tungstenite::Message> + Unpin,
    <S as futures::Sink<tungstenite::Message>>::Error: std::fmt::Debug,
{
    let claim = to_json(
        "claim",
        MessageKind::Control(ControlMessage::ClaimHost {
            secret: secret.to_string(),
        }),
    );
    sink.send(tungstenite::Message::Text(claim.into()))
        .await
        .expect("send ClaimHost");
}

/// Read frames until a Close, returning its code as a `u16` (0 if the peer
/// dropped without a close frame).
async fn recv_close_code<S>(stream: &mut S) -> u16
where
    S: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    timeout(TIMEOUT, async {
        loop {
            match stream.next().await {
                Some(Ok(tungstenite::Message::Close(frame))) => {
                    return frame.map_or(0, |f| u16::from(f.code));
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return 0,
            }
        }
    })
    .await
    .expect("recv close timed out")
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Full relay round-trip: host creates session → guest connects → guest
/// mutation relayed to host → host event relayed to guest → host ends
/// session → guest receives `SessionEnded` and is disconnected.
#[tokio::test]
async fn relay_integration_round_trip() {
    let state = test_state();
    let base = start_server(state).await;

    // 1. Host connects.
    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host ws connect");
    let (mut host_sink, mut host_stream) = host_ws.split();
    send_claim(&mut host_sink, "host-secret").await;

    // 2. Host sends CreateSession with read+write permissions.
    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions {
                read: true,
                write: true,
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();

    // 3. Host receives SessionCreated with token.
    let resp_json = recv_text(&mut host_stream).await;
    let resp = parse_msg(&resp_json);
    assert_eq!(resp.id, "ctrl-1");
    let (session_id, token) = match resp.kind {
        MessageKind::Control(ControlMessage::SessionCreated { session_id, token }) => {
            (session_id, token)
        }
        other => panic!("expected SessionCreated, got {other:?}"),
    };
    assert!(!session_id.is_empty());
    assert!(!token.is_empty());

    // 4. Guest connects with token.
    let guest_url = format!("{base}/ws/connect?token={token}");
    let (guest_ws, _) = tokio_tungstenite::connect_async(&guest_url)
        .await
        .expect("guest ws connect");
    let (mut guest_sink, mut guest_stream) = guest_ws.split();

    // Host should receive GuestConnected notification.
    let connected_json = recv_text(&mut host_stream).await;
    let connected = parse_msg(&connected_json);
    match connected.kind {
        MessageKind::Control(ControlMessage::GuestConnected { client_id }) => {
            assert!(
                client_id.0.starts_with("agent:"),
                "expected agent client id, got {}",
                client_id.0
            );
        }
        other => panic!("expected GuestConnected, got {other:?}"),
    }

    // 5. Guest sends a mutation → host should receive it.
    let mutation = to_json(
        "m-1",
        MessageKind::Mutation(Mutation::CellAdd {
            cell: NewCell {
                source: "let x = 42;".to_string(),
                cell_type: CellType::Code,
                label: "Test Cell".to_string(),
                cargo_toml: None,
                shared: false,
            },
            after_cell_id: None,
        }),
    );
    guest_sink
        .send(tungstenite::Message::Text(mutation.into()))
        .await
        .unwrap();

    let host_recv = recv_text(&mut host_stream).await;
    let host_msg = parse_msg(&host_recv);
    assert_eq!(host_msg.id, "m-1");
    assert!(
        matches!(
            host_msg.kind,
            MessageKind::Mutation(Mutation::CellAdd { .. })
        ),
        "expected CellAdd mutation, got {:?}",
        host_msg.kind
    );

    // 6. Host sends an event → guest should receive it.
    let event = to_json(
        "evt-1",
        MessageKind::Event(EventEnvelope {
            by: ClientId::browser(),
            event: Event::CellDeleted {
                cell_id: "c-99".to_string(),
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(event.into()))
        .await
        .unwrap();

    let guest_recv = recv_text(&mut guest_stream).await;
    let guest_msg = parse_msg(&guest_recv);
    assert_eq!(guest_msg.id, "evt-1");
    match guest_msg.kind {
        MessageKind::Event(envelope) => {
            assert_eq!(envelope.by, ClientId::browser());
            assert!(
                matches!(envelope.event, Event::CellDeleted { ref cell_id } if cell_id == "c-99")
            );
        }
        other => panic!("expected Event, got {other:?}"),
    }

    // 7. Host ends session → guest receives SessionEnded.
    let end_session = to_json(
        "ctrl-2",
        MessageKind::Control(ControlMessage::EndSession {
            session_id: session_id.clone(),
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(end_session.into()))
        .await
        .unwrap();

    // Guest should receive SessionEnded.
    let ended_json = recv_text(&mut guest_stream).await;
    let ended_msg = parse_msg(&ended_json);
    match ended_msg.kind {
        MessageKind::Control(ControlMessage::SessionEnded {
            session_id: ended_sid,
        }) => {
            assert_eq!(ended_sid, session_id);
        }
        other => panic!("expected SessionEnded, got {other:?}"),
    }

    // Guest should be disconnected (stream closes).
    let disconnect = timeout(TIMEOUT, guest_stream.next()).await;
    match disconnect {
        Ok(Some(Ok(tungstenite::Message::Close(_)) | Err(_)) | None) => {}
        other => panic!("expected guest disconnect, got {other:?}"),
    }
}

/// Invalid token is refused with HTTP 401.
#[tokio::test]
async fn guest_connect_invalid_token_rejected() {
    let state = test_state();
    let base = start_server(state).await;

    let url = format!("{base}/ws/connect?token=bogus-token");
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("expected connection to be rejected");

    assert_eq!(
        http_status(&err),
        Some(401),
        "invalid token should be HTTP 401, got {err:?}"
    );
}

/// Expired token is refused with HTTP 410 (the `SessionExpired` → GONE branch).
#[tokio::test]
async fn guest_connect_expired_token_returns_410() {
    let state = test_state();
    // Share the session store with the running server before moving state in.
    let store = state.ws.sessions.clone();
    let base = start_server(state).await;

    let result = store
        .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
        .await;
    assert!(store.expire_session(&result.session_id).await);

    let url = format!("{base}/ws/connect?token={}", result.token);
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("expected expired token to be rejected");

    assert_eq!(
        http_status(&err),
        Some(410),
        "expired token should be HTTP 410, got {err:?}"
    );
}

/// A permission-denied mutation is answered over the wire with a
/// `PermissionDenied` error and never reaches the host.
#[tokio::test]
async fn permission_denied_mutation_replies_with_error() {
    let state = test_state();
    let base = start_server(state).await;

    // Host connects and creates a READ-ONLY session (write denied).
    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host ws connect");
    let (mut host_sink, mut host_stream) = host_ws.split();
    send_claim(&mut host_sink, "host-secret").await;

    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions {
                read: true,
                write: false,
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();

    let token = match parse_msg(&recv_text(&mut host_stream).await).kind {
        MessageKind::Control(ControlMessage::SessionCreated { token, .. }) => token,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Guest connects and attempts a mutation it isn't allowed to make.
    let guest_url = format!("{base}/ws/connect?token={token}");
    let (guest_ws, _) = tokio_tungstenite::connect_async(&guest_url)
        .await
        .expect("guest ws connect");
    let (mut guest_sink, mut guest_stream) = guest_ws.split();

    // Host observes GuestConnected first.
    let _ = recv_text(&mut host_stream).await;

    let mutation = to_json(
        "m-1",
        MessageKind::Mutation(Mutation::CellReorder { cell_ids: vec![] }),
    );
    guest_sink
        .send(tungstenite::Message::Text(mutation.into()))
        .await
        .unwrap();

    // Guest receives a PermissionDenied error for that mutation id.
    let err = parse_msg(&recv_text(&mut guest_stream).await);
    assert_eq!(err.id, "m-1");
    match err.kind {
        MessageKind::Response(Response::Error { code, .. }) => {
            assert_eq!(code, ErrorCode::PermissionDenied);
        }
        other => panic!("expected PermissionDenied error, got {other:?}"),
    }

    // Host must not have received the denied mutation.
    let leaked = timeout(Duration::from_millis(300), host_stream.next()).await;
    assert!(
        leaked.is_err(),
        "host must not receive a permission-denied mutation, got {leaked:?}"
    );
}

/// When the host disconnects, its sessions are invalidated and connected guests
/// receive `SessionEnded` and are then disconnected.
#[tokio::test]
async fn host_disconnect_ends_guest_session() {
    let state = test_state();
    let base = start_server(state).await;

    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host ws connect");
    let (mut host_sink, mut host_stream) = host_ws.split();
    send_claim(&mut host_sink, "host-secret").await;

    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions::default(),
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();

    let (session_id, token) = match parse_msg(&recv_text(&mut host_stream).await).kind {
        MessageKind::Control(ControlMessage::SessionCreated { session_id, token }) => {
            (session_id, token)
        }
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    let guest_url = format!("{base}/ws/connect?token={token}");
    let (guest_ws, _) = tokio_tungstenite::connect_async(&guest_url)
        .await
        .expect("guest ws connect");
    let (_guest_sink, mut guest_stream) = guest_ws.split();

    // Host observes GuestConnected.
    let _ = recv_text(&mut host_stream).await;

    // Host disconnects abruptly (drop both halves of its socket) → the server
    // invalidates the session and notifies the guest.
    drop(host_sink);
    drop(host_stream);

    // Guest receives SessionEnded for its session.
    let ended = parse_msg(&recv_text(&mut guest_stream).await);
    match ended.kind {
        MessageKind::Control(ControlMessage::SessionEnded {
            session_id: ended_sid,
        }) => assert_eq!(ended_sid, session_id),
        other => panic!("expected SessionEnded, got {other:?}"),
    }

    // ...and is then disconnected.
    let disconnect = timeout(TIMEOUT, guest_stream.next()).await;
    match disconnect {
        Ok(Some(Ok(tungstenite::Message::Close(_)) | Err(_)) | None) => {}
        other => panic!("expected guest disconnect, got {other:?}"),
    }
}

/// A `read: false` (write-only) guest must not receive content-bearing broadcast
/// events — the `read` permission is a confidentiality boundary end-to-end.
#[tokio::test]
async fn read_denied_guest_does_not_receive_content_events() {
    let state = test_state();
    let base = start_server(state).await;

    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host ws connect");
    let (mut host_sink, mut host_stream) = host_ws.split();
    send_claim(&mut host_sink, "host-secret").await;

    // Write-only session: read denied.
    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions {
                read: false,
                write: true,
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();

    let token = match parse_msg(&recv_text(&mut host_stream).await).kind {
        MessageKind::Control(ControlMessage::SessionCreated { token, .. }) => token,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    let guest_url = format!("{base}/ws/connect?token={token}");
    let (guest_ws, _) = tokio_tungstenite::connect_async(&guest_url)
        .await
        .expect("guest ws connect");
    let (_guest_sink, mut guest_stream) = guest_ws.split();
    let _ = recv_text(&mut host_stream).await; // GuestConnected

    // Host broadcasts a content-bearing event.
    let event = to_json(
        "evt-1",
        MessageKind::Event(EventEnvelope {
            by: ClientId::browser(),
            event: Event::CellDeleted {
                cell_id: "c-1".to_string(),
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(event.into()))
        .await
        .unwrap();

    // The read:false guest must receive nothing.
    let got = timeout(Duration::from_millis(300), guest_stream.next()).await;
    assert!(
        got.is_err(),
        "a read:false guest must not receive content events, got {got:?}"
    );
}

/// A write-only (`read: false`) guest must still receive the ack for a mutation
/// it originated — otherwise its client strands on the request timeout despite
/// the mutation succeeding — while never receiving content events it did not
/// originate. Confirms the confidentiality boundary and the ack path coexist.
#[tokio::test]
async fn write_only_guest_receives_ack_but_no_foreign_content() {
    let state = test_state();
    let base = start_server(state).await;

    // Host connects and creates a WRITE-ONLY session (read denied).
    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host ws connect");
    let (mut host_sink, mut host_stream) = host_ws.split();
    send_claim(&mut host_sink, "host-secret").await;

    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions {
                read: false,
                write: true,
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();

    let token = match parse_msg(&recv_text(&mut host_stream).await).kind {
        MessageKind::Control(ControlMessage::SessionCreated { token, .. }) => token,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Guest connects with the write-only token.
    let guest_url = format!("{base}/ws/connect?token={token}");
    let (guest_ws, _) = tokio_tungstenite::connect_async(&guest_url)
        .await
        .expect("guest ws connect");
    let (mut guest_sink, mut guest_stream) = guest_ws.split();
    let _ = recv_text(&mut host_stream).await; // GuestConnected

    // Guest sends a mutation it is allowed to make.
    let mutation = to_json(
        "m-1",
        MessageKind::Mutation(Mutation::CellReorder {
            cell_ids: vec!["a".into()],
        }),
    );
    guest_sink
        .send(tungstenite::Message::Text(mutation.into()))
        .await
        .unwrap();

    // Host receives it, then emits (a) an unrelated browser-originated content
    // event the write-only guest must never see, and (b) the confirming Event
    // for the guest's own mutation, carrying its id.
    let relayed = parse_msg(&recv_text(&mut host_stream).await);
    assert_eq!(relayed.id, "m-1", "host must receive the guest mutation");

    let foreign = to_json(
        "",
        MessageKind::Event(EventEnvelope {
            by: ClientId::browser(),
            event: Event::CellDeleted {
                cell_id: "foreign".into(),
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(foreign.into()))
        .await
        .unwrap();

    let ack = to_json(
        "m-1",
        MessageKind::Event(EventEnvelope {
            by: ClientId::agent("agent"),
            event: Event::CellReordered {
                cell_ids: vec!["a".into()],
            },
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(ack.into()))
        .await
        .unwrap();

    // The write-only guest's first (and only) frame is its own ack: the foreign
    // content event was gated out, yet the ack still arrived despite read:false.
    let got = parse_msg(&recv_text(&mut guest_stream).await);
    assert_eq!(got.id, "m-1", "originator must receive its mutation ack");
    match got.kind {
        MessageKind::Event(env) => assert!(
            matches!(env.event, Event::CellReordered { .. }),
            "expected the mutation's confirming event, got {:?}",
            env.event
        ),
        other => panic!("expected an Event ack, got {other:?}"),
    }

    // No further frames — the foreign content event never reached the guest.
    let leaked = timeout(Duration::from_millis(300), guest_stream.next()).await;
    assert!(
        leaked.is_err(),
        "write-only guest must not receive foreign content events, got {leaked:?}"
    );
}

/// A guest that sends nothing is reaped by the idle timeout.
#[tokio::test]
async fn guest_idle_timeout_closes_stale_connection() {
    // A tiny idle timeout so we don't wait out the production value.
    let ws = WsState::default().with_guest_idle_timeout(Duration::from_millis(200));
    let store = ws.sessions.clone();
    let base = start_server(state_with_ws(ws)).await;

    let result = store
        .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
        .await;

    let url = format!("{base}/ws/connect?token={}", result.token);
    let (guest_ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("guest ws connect");
    let (_sink, mut guest_stream) = guest_ws.split();

    // Send nothing: the idle timeout should reap the connection.
    let closed = timeout(TIMEOUT, guest_stream.next())
        .await
        .expect("idle timeout should close the connection well within TIMEOUT");
    match closed {
        Some(Ok(tungstenite::Message::Close(_)) | Err(_)) | None => {}
        other => panic!("expected idle disconnect, got {other:?}"),
    }
}

/// A guest connection over the global cap is refused with HTTP 503.
#[tokio::test]
async fn guest_connection_cap_rejects_over_limit() {
    let ws = WsState::default().with_max_guests(2);
    let store = ws.sessions.clone();

    // Pre-fill the cap with two directly-registered guests (deterministic — no
    // over-the-wire registration race to observe).
    let (tx1, _r1) = mpsc::channel::<String>(8);
    let (tx2, _r2) = mpsc::channel::<String>(8);
    ws.register_guest("sess", "c1", tx1).await;
    ws.register_guest("sess", "c2", tx2).await;

    let base = start_server(state_with_ws(ws)).await;

    let result = store
        .create_session("nb-1".into(), "conn-1".into(), Permissions::default())
        .await;

    // The third connection must be rejected with HTTP 503 (cap reached).
    let url = format!("{base}/ws/connect?token={}", result.token);
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("expected the over-cap connection to be rejected");

    assert_eq!(
        http_status(&err),
        Some(503),
        "over-cap connection should be HTTP 503, got {err:?}"
    );
}

/// The host's first frame must be a `ClaimHost`; anything else is closed 4400
/// and never registered (PRD-0038 T-014).
#[tokio::test]
async fn host_first_frame_must_be_claim() {
    let state = test_state();
    let base = start_server(state).await;

    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host ws connect");
    let (mut host_sink, mut host_stream) = host_ws.split();

    // Skip the handshake — send CreateSession as the first frame.
    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions::default(),
        }),
    );
    host_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();

    // The socket is closed with 4400; no SessionCreated is ever delivered.
    assert_eq!(recv_close_code(&mut host_stream).await, 4400);
}

/// A second host claiming with the wrong secret is rejected (close 4403) and
/// does NOT evict the incumbent host (PRD-0038 T-014).
#[tokio::test]
async fn host_claim_mismatch_rejected_without_evicting_incumbent() {
    let state = test_state();
    let base = start_server(state).await;

    // Host 1 claims the notebook (TOFU) and creates a session so a guest can join.
    let host_url = format!("{base}/ws/host?notebook_id=test-nb");
    let (host1_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host1 ws connect");
    let (mut host1_sink, mut host1_stream) = host1_ws.split();
    send_claim(&mut host1_sink, "secret-A").await;

    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions::default(),
        }),
    );
    host1_sink
        .send(tungstenite::Message::Text(create_session.into()))
        .await
        .unwrap();
    let token = match parse_msg(&recv_text(&mut host1_stream).await).kind {
        MessageKind::Control(ControlMessage::SessionCreated { token, .. }) => token,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Host 2 connects and claims with the WRONG secret → rejected 4403.
    let (host2_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host2 ws connect");
    let (mut host2_sink, mut host2_stream) = host2_ws.split();
    send_claim(&mut host2_sink, "secret-B").await;
    assert_eq!(recv_close_code(&mut host2_stream).await, 4403);

    // Host 1 was NOT evicted: a guest mutation still routes to it.
    let (guest_ws, _) =
        tokio_tungstenite::connect_async(&format!("{base}/ws/connect?token={token}"))
            .await
            .expect("guest ws connect");
    let (mut guest_sink, _guest_stream) = guest_ws.split();
    let _ = recv_text(&mut host1_stream).await; // GuestConnected

    let mutation = to_json(
        "m-1",
        MessageKind::Mutation(Mutation::CellReorder {
            cell_ids: vec!["a".into()],
        }),
    );
    guest_sink
        .send(tungstenite::Message::Text(mutation.into()))
        .await
        .unwrap();
    let host_recv = parse_msg(&recv_text(&mut host1_stream).await);
    assert_eq!(
        host_recv.id, "m-1",
        "incumbent host must still receive guest traffic"
    );
}

/// Extract the HTTP status code from a tungstenite handshake error, if any.
fn http_status(err: &tungstenite::Error) -> Option<u16> {
    match err {
        tungstenite::Error::Http(resp) => Some(resp.status().as_u16()),
        _ => None,
    }
}
