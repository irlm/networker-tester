//! `/ws/testers` — project-scoped tester queue subscription WebSocket.
//!
//! Clients authenticate via `?token=<jwt>` (same pattern as `/ws/dashboard`),
//! then send `subscribe_tester_queue` / `unsubscribe_tester_queue` messages.
//! For each valid (project_id, tester_id) the handler:
//!  1. validates the user is a member of the project (`project_member`)
//!  2. validates the tester belongs to that project (`project_tester`)
//!  3. registers the connection with `TesterQueueHub`
//!  4. sends an initial `tester_queue_snapshot`
//!
//! Updates pushed by the hub are forwarded to the socket. Per-connection rate
//! limit: `DASHBOARD_MAX_SUB_MSGS_PER_MIN` subscribe/unsubscribe messages per
//! rolling 60s window (default 10).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use networker_common::tester_messages::TesterMessage;
use networker_dashboard::services::tester_queue_hub::TesterQueueHub;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::{self, Claims};
use crate::AppState;

const CHANNEL_BUF: usize = 64;
const DEFAULT_MAX_SUB_MSGS_PER_MIN: u32 = 10;

#[derive(Deserialize)]
struct TesterWsQuery {
    token: Option<String>,
}

async fn tester_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(q): Query<TesterWsQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let token = q.token.as_deref().unwrap_or("");
    if token.is_empty() {
        tracing::warn!("Tester WebSocket rejected: no token provided");
        return Err(StatusCode::UNAUTHORIZED);
    }
    match auth::validate_token(token, &state.jwt_secret) {
        Ok(claims) => {
            tracing::info!(
                email = %claims.email,
                user_id = %claims.sub,
                "Tester WebSocket authenticated"
            );
            Ok(ws.on_upgrade(move |socket| handle_tester_socket(socket, state, claims)))
        }
        Err(_) => {
            tracing::warn!("Tester WebSocket rejected: invalid token");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

async fn handle_tester_socket(socket: WebSocket, state: Arc<AppState>, claims: Claims) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (fwd_tx, mut fwd_rx) = mpsc::channel::<TesterMessage>(CHANNEL_BUF);
    let hub = state.tester_queue_hub.clone();

    // Active subscriptions on this socket: (project_id, tester_id) -> sub_id.
    let mut subs: HashMap<(String, String), u64> = HashMap::new();
    let mut bucket = SubMsgBucket::new();

    loop {
        tokio::select! {
            // Hub -> client: forward pushes to the WS sink.
            Some(msg) = fwd_rx.recv() => {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!(error = %e, "tester ws: serialize forwarded msg");
                        continue;
                    }
                };
                if ws_sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }

            // Client -> server: parse and handle.
            incoming = ws_stream.next() => {
                let Some(Ok(frame)) = incoming else { break; };
                let text = match frame {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    Message::Ping(p) => {
                        if ws_sink.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    Message::Pong(_) | Message::Binary(_) => continue,
                };

                let parsed: TesterMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!(error = %e, "tester ws: bad client message");
                        continue;
                    }
                };

                match parsed {
                    TesterMessage::SubscribeTesterQueue { project_id, tester_ids } => {
                        if !bucket.allow() {
                            tracing::debug!(
                                user_id = %claims.sub,
                                "tester ws: subscribe rate limit exceeded"
                            );
                            continue;
                        }
                        if let Err(e) = handle_subscribe(
                            &state,
                            &hub,
                            &claims,
                            &project_id,
                            &tester_ids,
                            &fwd_tx,
                            &mut subs,
                        )
                        .await
                        {
                            tracing::warn!(
                                error = %e,
                                user_id = %claims.sub,
                                project_id = %project_id,
                                "tester ws: subscribe failed"
                            );
                        }
                    }
                    TesterMessage::UnsubscribeTesterQueue { tester_ids } => {
                        if !bucket.allow() {
                            continue;
                        }
                        for tid in tester_ids {
                            let matching: Vec<(String, String)> = subs
                                .keys()
                                .filter(|(_, t)| t == &tid)
                                .cloned()
                                .collect();
                            for key in matching {
                                if let Some(sub_id) = subs.remove(&key) {
                                    hub.unsubscribe(&key.0, &key.1, sub_id).await;
                                }
                            }
                        }
                    }
                    // Server->client variants are not valid as inbound messages.
                    TesterMessage::TesterQueueSnapshot { .. }
                    | TesterMessage::TesterQueueUpdate { .. }
                    | TesterMessage::PhaseUpdate { .. } => {}
                }
            }
        }
    }

    // Cleanup: drop all subscriptions on disconnect.
    for ((pid, tid), sub_id) in subs.drain() {
        hub.unsubscribe(&pid, &tid, sub_id).await;
    }
}

async fn handle_subscribe(
    state: &Arc<AppState>,
    hub: &Arc<TesterQueueHub>,
    claims: &Claims,
    project_id: &str,
    tester_ids: &[String],
    fwd_tx: &mpsc::Sender<TesterMessage>,
    subs: &mut HashMap<(String, String), u64>,
) -> anyhow::Result<()> {
    let client = state.db.get().await?;

    // 1. Project membership.
    let is_member = if claims.is_platform_admin {
        true
    } else {
        crate::db::projects::get_member_role(&client, project_id, &claims.sub)
            .await?
            .is_some()
    };
    if !is_member {
        anyhow::bail!(
            "user {} is not a member of project {}",
            claims.sub,
            project_id
        );
    }

    // 2. Validate tester_ids belong to this project.
    let tid_uuids: Vec<Uuid> = tester_ids
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    if tid_uuids.is_empty() {
        return Ok(());
    }

    let rows = client
        .query(
            "SELECT tester_id FROM project_tester \
             WHERE project_id = $1 AND tester_id = ANY($2)",
            &[&project_id, &tid_uuids],
        )
        .await?;
    let valid_ids: Vec<String> = rows
        .iter()
        .map(|r| r.get::<_, Uuid>(0).to_string())
        .collect();

    // 3. Register + snapshot.
    for tid in valid_ids {
        let key = (project_id.to_string(), tid.clone());
        if subs.contains_key(&key) {
            continue;
        }
        let (sub_id, seq) = hub.subscribe(project_id, &tid, fwd_tx.clone()).await?;
        subs.insert(key, sub_id);

        // MVP snapshot: empty running/queued. Task 22 will populate from DB.
        let snapshot = TesterMessage::TesterQueueSnapshot {
            project_id: project_id.to_string(),
            tester_id: tid,
            seq,
            running: None,
            queued: vec![],
        };
        let _ = fwd_tx.send(snapshot).await;
    }

    Ok(())
}

/// Sliding-window rate limiter for subscribe/unsubscribe client messages.
struct SubMsgBucket {
    timestamps: Vec<Instant>,
    cap: u32,
}

impl SubMsgBucket {
    fn new() -> Self {
        let cap = std::env::var("DASHBOARD_MAX_SUB_MSGS_PER_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_SUB_MSGS_PER_MIN);
        Self {
            timestamps: Vec::new(),
            cap,
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        self.timestamps
            .retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        if self.timestamps.len() as u32 >= self.cap {
            return false;
        }
        self.timestamps.push(now);
        true
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws/testers", get(tester_ws_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use networker_common::tester_messages::QueueEntry;

    #[test]
    fn sub_msg_bucket_enforces_cap() {
        let mut b = SubMsgBucket {
            timestamps: Vec::new(),
            cap: 3,
        };
        assert!(b.allow());
        assert!(b.allow());
        assert!(b.allow());
        assert!(!b.allow(), "4th message should be rejected");
    }

    #[test]
    fn sub_msg_bucket_default_cap_from_env_or_10() {
        let b = SubMsgBucket::new();
        // Either env override or default 10.
        assert!(b.cap >= 1);
    }

    #[tokio::test]
    async fn hub_forwards_updates_to_subscriber_channel() {
        // End-to-end without the socket layer: subscribe an mpsc, notify via
        // hub, assert the forwarded message lands in the same channel that
        // handle_tester_socket reads from.
        let hub = Arc::new(TesterQueueHub::new());
        let (tx, mut rx) = mpsc::channel::<TesterMessage>(CHANNEL_BUF);

        let (_id, _seq) = hub.subscribe("proj-1", "tester-1", tx).await.unwrap();

        hub.notify(
            "proj-1",
            "tester-1",
            "benchmark_queued",
            None,
            vec![QueueEntry {
                config_id: "cfg-1".into(),
                name: "bench".into(),
                position: Some(0),
                eta_seconds: None,
            }],
        )
        .await;

        let got = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("hub should deliver within timeout")
            .expect("channel should yield message");

        match got {
            TesterMessage::TesterQueueUpdate {
                project_id,
                tester_id,
                trigger,
                queued,
                ..
            } => {
                assert_eq!(project_id, "proj-1");
                assert_eq!(tester_id, "tester-1");
                assert_eq!(trigger, "benchmark_queued");
                assert_eq!(queued.len(), 1);
            }
            other => panic!("expected TesterQueueUpdate, got {other:?}"),
        }
    }
}
