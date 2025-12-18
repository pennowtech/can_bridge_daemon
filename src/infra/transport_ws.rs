use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::{
    app::BridgeService,
    domain::{
        frame::{Direction, FrameEvent},
        protocol::{ClientRequest, ServerResponse},
    },
};

pub struct WsJsonServer {
    addr: SocketAddr,
}

impl WsJsonServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    pub async fn run(self, service: BridgeService) -> Result<()> {
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .with_state(Arc::new(service))
            // IMPORTANT: enable ConnectInfo extraction
            .into_make_service_with_connect_info::<SocketAddr>();

        let listener = tokio::net::TcpListener::bind(self.addr)
            .await
            .with_context(|| format!("failed to bind WS listener on {}", self.addr))?;

        info!(addr=%self.addr, "ws server listening (/ws)");

        axum::serve(listener, app)
            .await
            .context("axum serve failed")?;

        Ok(())
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(service): State<Arc<BridgeService>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, peer, service))
}

async fn handle_ws(socket: WebSocket, peer: SocketAddr, service: Arc<BridgeService>) {
    info!(peer=%peer, "ws client connected");

    let (mut ws_tx, mut ws_rx) = socket.split();

    let subscribed_ifaces: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerResponse>();
    let mut frames_rx: broadcast::Receiver<FrameEvent> = service.subscribe_frames();

    let writer = {
        let subscribed_ifaces = subscribed_ifaces.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_resp = out_rx.recv() => {
                        let Some(resp) = maybe_resp else { break; };
                        let txt = match serde_json::to_string(&resp) {
                            Ok(s) => s,
                            Err(e) => {
                                warn!(error=%e, "ws: failed to serialize response");
                                continue;
                            }
                        };
                        if let Err(e) = ws_tx.send(Message::Text(txt)).await {
                            warn!(error=%e, "ws: write failed");
                            break;
                        }
                    }

                    ev = frames_rx.recv() => {
                        match ev {
                            Ok(ev) => {
                                if should_send_frame(&subscribed_ifaces, &ev).await {
                                    let resp = frame_event_to_response(ev);
                                    let txt = match serde_json::to_string(&resp) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            warn!(error=%e, "ws: failed to serialize frame");
                                            continue;
                                        }
                                    };
                                    if let Err(e) = ws_tx.send(Message::Text(txt)).await {
                                        warn!(error=%e, "ws: write frame failed");
                                        break;
                                    }
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!(dropped=%n, "ws: frame receiver lagged; dropped messages");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        })
    };

    // Hello handshake
    let hello = ServerResponse::Hello {
        version: "0.9".to_string(),
        features: vec!["ws".into(), "json".into(), "stream".into()],
        server_name: "can-bridge-daemon".to_string(),
    };

    if out_tx.send(hello).is_err() {
        warn!(peer=%peer, "ws: writer ended before hello");
        return;
    }

    let first = match tokio::time::timeout(Duration::from_secs(3), ws_rx.next()).await {
        Ok(v) => v,
        Err(_) => {
            warn!(peer=%peer, "ws: hello_ack timeout");
            return;
        }
    };

    let Some(first) = first else {
        warn!(peer=%peer, "ws: client closed before hello_ack");
        return;
    };

    let first = match first {
        Ok(m) => m,
        Err(e) => {
            warn!(peer=%peer, error=%e, "ws: receive error during handshake");
            return;
        }
    };

    let raw = match first {
        Message::Text(s) => s,
        Message::Binary(b) => match String::from_utf8(b) {
            Ok(s) => s,
            Err(_) => {
                warn!(peer=%peer, "ws: non-utf8 binary during handshake");
                return;
            }
        },
        Message::Close(_) => {
            info!(peer=%peer, "ws: closed during handshake");
            return;
        }
        _ => {
            warn!(peer=%peer, "ws: unexpected message type during handshake");
            return;
        }
    };

    debug!(peer=%peer, raw=%raw, "ws: rx handshake");

    let req: ClientRequest = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            let _ = out_tx.send(ServerResponse::Error {
                message: format!("invalid hello_ack json: {e}"),
            });
            return;
        }
    };

    match req {
        ClientRequest::HelloAck { client, protocol } => {
            info!(peer=%peer, client=%client, protocol=%protocol, "ws: hello_ack received");
        }
        other => {
            let _ = out_tx.send(ServerResponse::Error {
                message: format!("expected hello_ack, got: {other:?}"),
            });
            return;
        }
    }

    // Normal receive loop
    while let Some(next_msg) = ws_rx.next().await {
        let msg = match next_msg {
            Ok(m) => m,
            Err(e) => {
                warn!(peer=%peer, error=%e, "ws: receive error");
                break;
            }
        };

        let raw = match msg {
            Message::Text(s) => s,
            Message::Binary(b) => match String::from_utf8(b) {
                Ok(s) => s,
                Err(_) => {
                    let _ = out_tx.send(ServerResponse::Error {
                        message: "binary message must be utf-8 json".to_string(),
                    });
                    continue;
                }
            },
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };

        debug!(peer=%peer, raw=%raw, "ws: rx");

        let req: ClientRequest = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                let _ = out_tx.send(ServerResponse::Error {
                    message: format!("invalid json request: {e}"),
                });
                continue;
            }
        };

        debug!(peer=%peer, req=?req, "ws: parsed request");

        match req {
            ClientRequest::Subscribe { ifaces } => {
                {
                    let mut set = subscribed_ifaces.lock().await;
                    set.clear();
                    for i in &ifaces {
                        set.insert(i.clone());
                    }
                }
                let _ = out_tx.send(ServerResponse::Subscribed { ifaces });
                continue;
            }
            ClientRequest::Unsubscribe => {
                {
                    let mut set = subscribed_ifaces.lock().await;
                    set.clear();
                }
                let _ = out_tx.send(ServerResponse::Unsubscribed);
                continue;
            }
            _ => {}
        }

        let resp = service.handle(req).await;
        let _ = out_tx.send(resp);
    }

    drop(out_tx);
    let _ = writer.await;
    info!(peer=%peer, "ws client disconnected");
}

async fn should_send_frame(subscribed: &Arc<Mutex<HashSet<String>>>, ev: &FrameEvent) -> bool {
    let set = subscribed.lock().await;
    set.contains(&ev.iface)
}

fn frame_event_to_response(ev: FrameEvent) -> ServerResponse {
    ServerResponse::Frame {
        ts_ms: ev.ts_ms,
        iface: ev.iface,
        dir: match ev.dir {
            Direction::Rx => "rx".to_string(),
            Direction::Tx => "tx".to_string(),
        },
        id: ev.id,
        is_fd: ev.is_fd,
        data_hex: hex_lower(&ev.data),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize]);
        out.push(LUT[(b & 0x0F) as usize]);
    }
    String::from_utf8(out).unwrap_or_default()
}
