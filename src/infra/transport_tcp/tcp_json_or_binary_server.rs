use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use super::{tcp_binary, tcp_jsonl};
use crate::app::BridgeService;
use crate::domain::binary_message;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

pub struct TcpJsonOrBinaryServer {
    addr: SocketAddr,
}

impl TcpJsonOrBinaryServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    pub async fn run(self, service: BridgeService) -> Result<()> {
        let listener = TcpListener::bind(self.addr)
            .await
            .with_context(|| format!("tcp bind failed on {}", self.addr))?;

        info!(addr=%self.addr, "tcp server listening (jsonl or binary)");

        let service = Arc::new(service);

        loop {
            let (socket, peer) = listener.accept().await?;
            let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);

            info!(conn_id=%conn_id, peer=%peer, "client connected");

            let service = service.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(socket, peer, conn_id, service).await {
                    warn!(conn_id=%conn_id, peer=%peer, error=%e, "client handler error");
                }
                info!(conn_id=%conn_id, peer=%peer, "client handler ended");
            });
        }
    }
}

async fn handle_connection(
    socket: TcpStream,
    peer: SocketAddr,
    conn_id: u64,
    service: Arc<BridgeService>,
) -> Result<()> {
    let mut first_four_bytes = [0u8; 4];
    let bytes_peeked = socket.peek(&mut first_four_bytes).await.unwrap_or(0);

    if bytes_peeked == 4 && first_four_bytes == binary_message::MAGIC {
        info!(conn_id=%conn_id, peer=%peer, "tcp session mode=binary");
        tcp_binary::run_binary_session(socket, peer, conn_id, service).await
    } else {
        info!(conn_id=%conn_id, peer=%peer, "tcp session mode=jsonl");
        tcp_jsonl::run_jsonl_session(socket, peer, conn_id, service).await
    }
}
