use std::{net::SocketAddr, sync::Arc};
use tokio::net::{TcpListener, TcpStream};

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
use tracing::{info, warn};

use crate::{
    app::BridgeService,
    domain::{
        frame::{Direction, FrameEvent},
        protocol::{ClientRequest, DaemonResponse},
    },
};

use super::ws_binary::ws_binary_handler;
use super::ws_jsonl::ws_jsonl_handler;

pub struct WsJsonOrBinaryServer {
    addr: SocketAddr,
}

impl WsJsonOrBinaryServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    pub async fn run(self, service: BridgeService) -> Result<()> {
        let app = Router::new()
            // JSONL WebSocket (legacy / human readable)
            .route("/ws/text", get(ws_jsonl_handler))
            // Binary WebSocket (high performance)
            .route("/ws/binary", get(ws_binary_handler))
            .with_state(Arc::new(service))
            // IMPORTANT: enable ConnectInfo extraction
            .into_make_service_with_connect_info::<SocketAddr>();

        let listener = TcpListener::bind(self.addr)
            .await
            .with_context(|| format!("failed to bind WS listener on {}", self.addr))?;

        info!(addr=%self.addr, "ws server listening (/ws/text and /ws/binary)");

        axum::serve(listener, app)
            .await
            .context("axum serve failed")?;

        Ok(())
    }
}
