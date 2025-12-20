// SPDX-License-Identifier: Apache-2.0
//! tcp_binary
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
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{broadcast, mpsc, Mutex},
};
use tracing::{debug, info, warn};

use crate::{
    app::BridgeService,
    domain::{
        binary_message::{
            self, decode_client_from, encode_daemon, ClientBinaryRequest, DaemonBinaryResponse,
        },
        frame::{Direction, FrameEvent},
        protocol::{ClientRequest, DaemonResponse},
    },
};

pub async fn run_binary_session(
    socket: TcpStream,
    peer: SocketAddr,
    conn_id: u64,
    service: Arc<BridgeService>,
) -> Result<()> {
    let (mut read_half, mut write_half) = socket.into_split();

    let subscribed_ifaces: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut frames_receiver: broadcast::Receiver<FrameEvent> = service.subscribe_frames();

    // Outgoing daemon binary messages to writer task
    let (outgoing_sender, mut outgoing_receiver) =
        mpsc::unbounded_channel::<DaemonBinaryResponse>();

    let writer_task = {
        let subscribed_ifaces = subscribed_ifaces.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_resp = outgoing_receiver.recv() => {
                        let Some(response) = maybe_resp else { break; };
                        let packet = encode_daemon(&response)?;
                        write_half.write_all(&packet).await?;
                    }

                    maybe_event = frames_receiver.recv() => {
                        match maybe_event {
                            Ok(frame_event) => {
                                if should_send_frame(&subscribed_ifaces, &frame_event).await {
                                    let response = frame_event_to_daemon_binary(frame_event);
                                    let packet = encode_daemon(&response)?;
                                    write_half.write_all(&packet).await?;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(dropped)) => {
                                warn!(conn_id=%conn_id, dropped=%dropped, "binary: dropped frames due to lag");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            Result::<()>::Ok(())
        })
    };

    // Server-side binary handshake aligns with JSON:
    // 1) Client responds ClientHello
    // 2) Daemon sends HelloAck
    outgoing_sender
        .send(DaemonBinaryResponse::HelloAck {
            version: "1.0".to_string(),
            server_name: "can-bridge-daemon".to_string(),
            features: vec!["tcp".into(), "binary".into(), "jsonl".into()],
        })
        .map_err(|_| anyhow!("writer closed before binary hello_ack"))?;

    let mut receive_buffer: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut read_chunk = [0u8; 4096];

    // Expect ClientHello within 3 seconds
    let client_hello = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let bytes_read = read_half.read(&mut read_chunk).await?;
            if bytes_read == 0 {
                return Err(anyhow!("client closed during handshake"));
            }
            receive_buffer.extend_from_slice(&read_chunk[..bytes_read]);

            if let Some((request, consumed_bytes)) = decode_client_from(&receive_buffer)? {
                receive_buffer.drain(0..consumed_bytes);
                return Ok::<ClientBinaryRequest, anyhow::Error>(request);
            }
        }
    })
    .await
    .context("binary client_hello timeout")??;

    debug!(conn_id=%conn_id, peer=%peer, msg=?client_hello, "binary rx handshake");

    match client_hello {
        ClientBinaryRequest::ClientHello { client, protocol } => {
            info!(conn_id=%conn_id, peer=%peer, client=%client, protocol=%protocol, "binary client_hello received");
        }
        other => {
            let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                message: format!("expected client_hello, got {other:?}"),
            });
            drop(outgoing_sender);
            let _ = writer_task.await;
            return Ok(());
        }
    }

    // Main binary read/dispatch loop
    loop {
        // Decode all complete requests already buffered
        while let Some((request, consumed_bytes)) = decode_client_from(&receive_buffer)? {
            receive_buffer.drain(0..consumed_bytes);

            debug!(conn_id=%conn_id, peer=%peer, msg=?request, "binary rx");

            match request {
                ClientBinaryRequest::Ping { id } => {
                    let _ = outgoing_sender.send(DaemonBinaryResponse::Pong { id });
                }

                ClientBinaryRequest::ListIfaces => {
                    let response = service.handle(ClientRequest::ListIfaces).await;
                    match response {
                        DaemonResponse::Ifaces { items } => {
                            let _ = outgoing_sender.send(DaemonBinaryResponse::Ifaces { items });
                        }
                        DaemonResponse::Error { message } => {
                            let _ = outgoing_sender.send(DaemonBinaryResponse::Error { message });
                        }
                        other => {
                            let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                                message: format!("unexpected response for list_ifaces: {other:?}"),
                            });
                        }
                    }
                }

                ClientBinaryRequest::Subscribe { ifaces } => {
                    update_subscription_set(&subscribed_ifaces, ifaces.clone()).await;
                    let _ = outgoing_sender.send(DaemonBinaryResponse::Subscribed { ifaces });
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
                    // Keep app layer unchanged for now: it expects hex string.
                    let data_hex = bytes_to_hex_lower(&data);

                    let response = service
                        .handle(ClientRequest::SendFrame {
                            iface,
                            id,
                            is_fd,
                            brs,
                            esi,
                            data_hex,
                        })
                        .await;

                    match response {
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
                                message: format!("unexpected response for send_frame: {other:?}"),
                            });
                        }
                    }
                }

                // client_hello should only be sent once
                ClientBinaryRequest::ClientHello { .. } => {
                    let _ = outgoing_sender.send(DaemonBinaryResponse::Error {
                        message: "client_hello already completed".to_string(),
                    });
                }
            }
        }

        // Read more bytes from socket
        let bytes_read = read_half.read(&mut read_chunk).await?;
        if bytes_read == 0 {
            break;
        }
        receive_buffer.extend_from_slice(&read_chunk[..bytes_read]);
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
