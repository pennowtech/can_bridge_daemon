use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, info, warn};

use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};

use crate::{
    app::BridgeService,
    domain::{
        binary_message::{self, ClientBinaryRequest, DaemonBinaryResponse},
        frame::{Direction, FrameEvent},
        protocol::{ClientRequest, DaemonResponse},
    },
};

pub async fn ws_binary_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(service): State<Arc<BridgeService>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        info!(peer=%peer, "ws-binary client connected");
        if let Err(e) = handle_ws_binary(socket, peer, service).await {
            warn!(peer=%peer, error=%e, "ws-binary session ended with error");
        }
    })
}

/// Run a WebSocket session in **binary mode**.
/// Handshake:
/// - client sends ClientHello first
/// - daemon replies HelloAck
///
/// After handshake:
/// - client can send ListIfaces / Subscribe / Unsubscribe / Ping / SendFrame
/// - daemon streams FrameEvent for subscribed ifaces
async fn handle_ws_binary(
    socket: WebSocket,
    peer: SocketAddr,
    service: Arc<BridgeService>,
) -> Result<()> {
    info!(peer=%peer, "Handling connection from ws-binary client");
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let subscribed_ifaces: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut frames_receiver: broadcast::Receiver<FrameEvent> = service.subscribe_frames();

    // Outgoing messages to single writer task
    let (outgoing_sender, mut outgoing_receiver) =
        mpsc::unbounded_channel::<DaemonBinaryResponse>();

    // Writer task: the only place we send on ws_sender
    let writer_task = {
        let subscribed_ifaces = subscribed_ifaces.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_resp = outgoing_receiver.recv() => {
                        let Some(resp) = maybe_resp else { break; };
                        let packet = binary_message::encode_daemon(&resp)?;
                        ws_sender.send(Message::Binary(packet)).await
                            .map_err(|e| anyhow!("ws send: {e}"))?;
                    }

                    maybe_ev = frames_receiver.recv() => {
                        match maybe_ev {
                            Ok(frame_event) => {
                                if should_send_frame(&subscribed_ifaces, &frame_event).await {
                                    let resp = frame_event_to_daemon_binary(frame_event);
                                    let packet = binary_message::encode_daemon(&resp)?;
                                    ws_sender.send(Message::Binary(packet)).await
                                        .map_err(|e| anyhow!("ws send: {e}"))?;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(dropped)) => {
                                warn!(dropped=%dropped, "ws-binary: dropped frames due to lag");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            Result::<()>::Ok(())
        })
    };

    // ---------------- Handshake: wait for ClientHello ----------------
    let mut receive_buffer: Vec<u8> = Vec::with_capacity(8 * 1024);

    let handshake_request = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let maybe_msg = ws_receiver.next().await;
            let Some(Ok(msg)) = maybe_msg else {
                return Err(anyhow!("ws closed during handshake"));
            };

            match msg {
                Message::Binary(bytes) => {
                    receive_buffer.extend_from_slice(&bytes);
                    if let Some((req, consumed)) =
                        binary_message::decode_client_from(&receive_buffer)?
                    {
                        receive_buffer.drain(0..consumed);
                        return Ok::<ClientBinaryRequest, anyhow::Error>(req);
                    }
                }
                Message::Text(_) => {
                    // If a client sends Text here, it picked JSON mode, not binary.
                    return Err(anyhow!("expected binary ClientHello, got Text"));
                }
                Message::Close(_) => return Err(anyhow!("ws closed during handshake")),
                _ => {}
            }
        }
    })
    .await??;

    debug!(msg=?handshake_request, "ws-binary handshake rx");

    match handshake_request {
        ClientBinaryRequest::ClientHello { client, protocol } => {
            info!(client=%client, protocol=%protocol, "ws-binary ClientHello received");
        }
        other => {
            let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                message: format!("expected ClientHello, got {other:?}"),
            });
            drop(outgoing_sender);
            let _ = writer_task.await;
            return Ok(());
        }
    }

    // Send HelloAck
    outgoing_sender
        .send(DaemonBinaryResponse::HelloAck {
            version: "1.0".to_string(),
            server_name: "can-bridge-daemon".to_string(),
            features: vec!["ws".into(), "binary".into(), "json".into()],
        })
        .map_err(|_| anyhow!("writer closed before hello_ack"))?;

    // ---------------- Main loop: read requests ----------------
    loop {
        let maybe_msg = ws_receiver.next().await;
        let Some(Ok(msg)) = maybe_msg else {
            break;
        };

        match msg {
            Message::Binary(bytes) => {
                receive_buffer.extend_from_slice(&bytes);

                while let Some((request, consumed)) =
                    binary_message::decode_client_from(&receive_buffer)?
                {
                    receive_buffer.drain(0..consumed);
                    debug!(msg=?request, "ws-binary rx");

                    match request {
                        ClientBinaryRequest::Ping { id } => {
                            let _ = outgoing_sender.send(DaemonBinaryResponse::Pong { id });
                        }

                        ClientBinaryRequest::ListIfaces => {
                            let resp = service.handle(ClientRequest::ListIfaces).await;
                            match resp {
                                DaemonResponse::Ifaces { items } => {
                                    let _ = outgoing_sender
                                        .send(DaemonBinaryResponse::Ifaces { items });
                                }
                                DaemonResponse::Error { message } => {
                                    let _ = outgoing_sender
                                        .send(DaemonBinaryResponse::Error { message });
                                }
                                other => {
                                    let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                                        message: format!(
                                            "unexpected list_ifaces response: {other:?}"
                                        ),
                                    });
                                }
                            }
                        }

                        ClientBinaryRequest::Subscribe { ifaces } => {
                            update_subscription_set(&subscribed_ifaces, ifaces.clone()).await;
                            let _ =
                                outgoing_sender.send(DaemonBinaryResponse::Subscribed { ifaces });
                        }

                        ClientBinaryRequest::Unsubscribe => {
                            clear_subscription_set(&subscribed_ifaces).await;
                            let _ = outgoing_sender.send(DaemonBinaryResponse::Unsubscribed);
                        }

                        ClientBinaryRequest::SendFrame {
                            iface,
                            id,
                            is_fd,
                            brs,
                            esi,
                            data,
                        } => {
                            let data_hex = bytes_to_hex_lower(&data);

                            let resp = service
                                .handle(ClientRequest::SendFrame {
                                    iface,
                                    id,
                                    is_fd,
                                    brs,
                                    esi,
                                    data_hex,
                                })
                                .await;

                            match resp {
                                DaemonResponse::SendAck { ok, error_message } => {
                                    let _ = outgoing_sender
                                        .send(DaemonBinaryResponse::SendAck { ok, error_message });
                                }
                                DaemonResponse::Error { message } => {
                                    let _ = outgoing_sender.send(DaemonBinaryResponse::SendAck {
                                        ok: false,
                                        error_message: Some(message),
                                    });
                                }
                                other => {
                                    let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                                        message: format!(
                                            "unexpected send_frame response: {other:?}"
                                        ),
                                    });
                                }
                            }
                        }

                        // ClientHello must only happen once
                        ClientBinaryRequest::ClientHello { .. } => {
                            let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                                message: "ClientHello already completed".to_string(),
                            });
                        }
                    }
                }
            }

            Message::Text(_) => {
                // In binary mode we reject Text to keep things deterministic.
                let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                    message: "unexpected Text in binary mode".to_string(),
                });
            }

            Message::Close(_) => break,
            _ => {}
        }
    }

    drop(outgoing_sender);
    let _ = writer_task.await;
    info!(peer=%peer, "ws client disconnected");
    Ok(())
}

// ---------------- helpers ----------------

async fn should_send_frame(
    subscribed: &Arc<Mutex<HashSet<String>>>,
    frame_event: &FrameEvent,
) -> bool {
    let set = subscribed.lock().await;
    set.contains(&frame_event.iface)
}

async fn update_subscription_set(subscribed: &Arc<Mutex<HashSet<String>>>, ifaces: Vec<String>) {
    let mut set = subscribed.lock().await;
    set.clear();
    for iface in ifaces {
        set.insert(iface);
    }
}

async fn clear_subscription_set(subscribed: &Arc<Mutex<HashSet<String>>>) {
    subscribed.lock().await.clear();
}

fn frame_event_to_daemon_binary(frame_event: FrameEvent) -> DaemonBinaryResponse {
    DaemonBinaryResponse::FrameEvent {
        ts_ms: frame_event.ts_ms,
        iface: frame_event.iface,
        dir: match frame_event.dir {
            Direction::Rx => "rx".to_string(),
            Direction::Tx => "tx".to_string(),
        },
        id: frame_event.id,
        is_fd: frame_event.is_fd,
        data: frame_event.data,
    }
}

fn bytes_to_hex_lower(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize]);
        out.push(LUT[(b & 0x0F) as usize]);
    }
    String::from_utf8(out).unwrap_or_default()
}
