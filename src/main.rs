// SPDX-License-Identifier: Apache-2.0
//! main
//!
//! Layer: Composition Root
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{info, warn};

use crate::ports::discovery::DiscoveryPort;

mod app;
mod domain;
mod infra;
mod ports;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:9500")]
    tcp_bind: String,

    /// WebSocket bind address, e.g. 127.0.0.1:9501 (path is /ws)
    #[arg(long, default_value = "127.0.0.1:9501")]
    ws_bind: String,

    /// gRPC bind address, e.g. 127.0.0.1:9502
    #[arg(long, default_value = "127.0.0.1:9502")]
    grpc_bind: String,

    /// Also run the fake generator (useful if your bus is quiet).
    #[arg(long)]
    fake: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!(bind=%args.tcp_bind, "can_bridge_daemon starting");

    // Choose discovery implementation:
    let discovery: Arc<dyn DiscoveryPort> =
        Arc::new(infra::iface_discovery_netlink::NetlinkDiscovery::new());
    // let discovery = Arc::new(infra::iface_discovery_dummy::StubDiscovery::new());

    // Choose CAN TX implementation:
    // (for now, only SocketCAN TX is implemented)
    let can_tx: Arc<dyn crate::ports::can_tx::CanTxPort> =
        Arc::new(infra::socketcan_tx::SocketCanTx::new());

    // App service + event bus
    let service = app::BridgeService::new(discovery.clone(), can_tx);

    // Discover CAN ifaces once at startup
    let ifaces = match discovery.list_can_ifaces().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error=%e, "iface discovery failed; starting fake generator with no ifaces");
            Vec::new()
        }
    };

    info!(ifaces=?ifaces, "discovered ifaces");

    // start REAL SocketCAN RX
    #[cfg(target_os = "linux")]
    {
        infra::socketcan_rx::start_socketcan_rx(service.clone(), ifaces.clone());
    }

    // Optional: also run fake frames to validate pipeline even if bus is quiet
    if args.fake {
        infra::fake_generator::start_fake_generator(service.clone(), ifaces.clone());
    }

    // Start TCP server
    let server = infra::transport_tcp::TcpJsonlServer::new(args.tcp_bind.parse()?);
    let server_handle = tokio::spawn({
        let service = service.clone();
        async move {
            if let Err(e) = server.run(service).await {
                warn!(error=%e, "tcp server exited with error");
            }
        }
    });

    // WebSocket server
    let ws_server = infra::transport_ws::WsJsonServer::new(args.ws_bind.parse()?);
    let ws_handle = tokio::spawn({
        let service = service.clone();
        async move {
            if let Err(e) = ws_server.run(service).await {
                warn!(error=%e, "ws server exited with error");
            }
        }
    });

    // gRPC server
    let grpc_server = infra::transport_grpc::GrpcServer::new(args.grpc_bind.parse()?);
    let grpc_handle = tokio::spawn({
        let service = service.clone();
        async move {
            if let Err(e) = grpc_server.run(service).await {
                warn!(error=%e, "grpc server exited with error");
            }
        }
    });

    info!("daemon running (terminate with Ctrl+C)");
    tokio::signal::ctrl_c().await?;
    info!("Ctrl+C received; shutting down");
    server_handle.abort();
    ws_handle.abort();
    grpc_handle.abort();
    Ok(())
}
