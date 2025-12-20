// SPDX-License-Identifier: Apache-2.0
//! tcp_jsonl
//!
//! Layer: Infrastructure
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{anyhow, Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::{broadcast, mpsc, Mutex},
};
use tracing::{debug, info, warn};

use crate::{
    app::BridgeService,
    domain::{
        frame::{Direction, FrameEvent},
        protocol::{ClientRequest, DaemonResponse},
    },
};

pub async fn run_jsonl_session(
    socket: TcpStream,
    peer: SocketAddr,
    conn_id: u64,
    service: Arc<BridgeService>,
) -> Result<()> {
    let (read_half, mut write_half) = socket.into_split();
    let mut line_reader = BufReader::new(read_half).lines();

    let subscribed_ifaces: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut frames_receiver: broadcast::Receiver<FrameEvent> = service.subscribe_frames();
    let (outgoing_sender, mut outgoing_receiver) = mpsc::unbounded_channel::<DaemonResponse>();

    let writer_task = {
        let subscribed_ifaces = subscribed_ifaces.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_resp = outgoing_receiver.recv() => {
                        let Some(response) = maybe_resp else { break; };
                        let line = serde_json::to_string(&response)
                            .map_err(|e| anyhow!("json serialize: {e}"))?;
                        write_half.write_all(line.as_bytes()).await?;
                        write_half.write_all(b"\n").await?;
                    }

                    maybe_event = frames_receiver.recv() => {
                        match maybe_event {
                            Ok(frame_event) => {
                                if should_send_frame(&subscribed_ifaces, &frame_event).await {
                                    let response = frame_event_to_json_response(frame_event);
                                    let line = serde_json::to_string(&response)
                                        .map_err(|e| anyhow!("json serialize frame: {e}"))?;
                                    write_half.write_all(line.as_bytes()).await?;
                                    write_half.write_all(b"\n").await?;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(dropped)) => {
                                warn!(conn_id=%conn_id, dropped=%dropped, "jsonl: dropped frames due to lag");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            Result::<()>::Ok(())
        })
    };

    // Daemon sends HelloAck first (JSON behavior)
    outgoing_sender
        .send(DaemonResponse::HelloAck {
            version: "1.0".to_string(),
            features: vec!["tcp".into(), "jsonl".into(), "binary".into()],
            server_name: "can-bridge-daemon".to_string(),
        })
        .map_err(|_| anyhow!("writer closed before hello_ack"))?;

    // Expect client_hello within 3 seconds
    let first_line = tokio::time::timeout(Duration::from_secs(3), line_reader.next_line())
        .await
        .context("client_hello timeout")??;

    let Some(first_line) = first_line else {
        return Ok(());
    };

    debug!(conn_id=%conn_id, peer=%peer, raw=%first_line, "jsonl rx handshake");

    let handshake: ClientRequest =
        serde_json::from_str(&first_line).context("parse client_hello")?;
    match handshake {
        ClientRequest::ClientHello { client, protocol } => {
            info!(conn_id=%conn_id, peer=%peer, client=%client, protocol=%protocol, "client_hello received");
        }
        other => {
            let _ = outgoing_sender.send(DaemonResponse::Error {
                message: format!("expected client_hello, got {other:?}"),
            });
            return Ok(());
        }
    }

    while let Some(line) = line_reader.next_line().await? {
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }

        debug!(conn_id=%conn_id, peer=%peer, raw=%raw, "jsonl rx");

        let request: ClientRequest = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                let _ = outgoing_sender.send(DaemonResponse::Error {
                    message: format!("invalid json: {e}"),
                });
                continue;
            }
        };

        match request {
            ClientRequest::Subscribe { ifaces } => {
                update_subscription_set(&subscribed_ifaces, ifaces.clone()).await;
                let _ = outgoing_sender.send(DaemonResponse::Subscribed { ifaces });
            }
            ClientRequest::Unsubscribe => {
                clear_subscription_set(&subscribed_ifaces).await;
                let _ = outgoing_sender.send(DaemonResponse::Unsubscribed);
            }
            ClientRequest::ClientHello { .. } => {
                let _ = outgoing_sender.send(DaemonResponse::Error {
                    message: "client_hello already completed".to_string(),
                });
            }
            other => {
                let response = service.handle(other).await;
                let _ = outgoing_sender.send(response);
            }
        }
    }

    drop(outgoing_sender);
    let _ = writer_task.await;
    Ok(())
}

async fn should_send_frame(
    subscribed_ifaces: &Arc<Mutex<HashSet<String>>>,
    frame_event: &FrameEvent,
) -> bool {
    let set = subscribed_ifaces.lock().await;
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

fn frame_event_to_json_response(frame_event: FrameEvent) -> DaemonResponse {
    DaemonResponse::Frame {
        ts_ms: frame_event.ts_ms,
        iface: frame_event.iface,
        dir: match frame_event.dir {
            Direction::Rx => "rx".to_string(),
            Direction::Tx => "tx".to_string(),
        },
        id: frame_event.id,
        is_fd: frame_event.is_fd,
        data_hex: bytes_to_hex_lower(&frame_event.data),
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
