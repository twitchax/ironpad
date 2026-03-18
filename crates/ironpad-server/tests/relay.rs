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
    self, ClientId, ControlMessage, Event, EventEnvelope, MessageKind, Mutation, NewCell,
    Permissions,
};
use ironpad_common::types::CellType;
use ironpad_common::AppConfig;
use ironpad_server::state::{AppState, WsState};
use ironpad_server::ws;
use leptos::config::LeptosOptions;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite;

// ── Helpers ─────────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(5);

/// Build a minimal `AppState` (no Leptos SSR routes needed).
fn test_state() -> AppState {
    AppState {
        leptos_options: LeptosOptions::builder().output_name("ironpad-test").build(),
        config: AppConfig {
            data_dir: PathBuf::from("/tmp"),
            cache_dir: PathBuf::from("/tmp"),
            port: 0,
            ironpad_cell_path: PathBuf::from("/tmp"),
            compilation_proxy: None,
        },
        ws: WsState::default(),
    }
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

    // 2. Host sends CreateSession with read+write permissions.
    let create_session = to_json(
        "ctrl-1",
        MessageKind::Control(ControlMessage::CreateSession {
            permissions: Permissions {
                read: true,
                write: true,
                execute: false,
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

/// Invalid token returns HTTP 401 (connection refused).
#[tokio::test]
async fn guest_connect_invalid_token_rejected() {
    let state = test_state();
    let base = start_server(state).await;

    let url = format!("{base}/ws/connect?token=bogus-token");
    let result = tokio_tungstenite::connect_async(&url).await;

    assert!(result.is_err(), "expected connection to be rejected");
}
