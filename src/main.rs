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
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::infra::transport_tcp::tcp_json_or_binary_server::TcpJsonOrBinaryServer;
use crate::infra::transport_ws::ws_json_or_binary_server::WsJsonOrBinaryServer;
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

    /// WebSocket bind address, e.g. 127.0.0.1:9501 (paths: /ws/text, /ws/binary)
    #[arg(long, default_value = "127.0.0.1:9501")]
    ws_bind: String,

    /// gRPC bind address, e.g. 127.0.0.1:9502
    #[arg(long, default_value = "127.0.0.1:9502")]
    grpc_bind: String,

    /// Also run the fake generator (useful if bus is quiet).
    #[arg(long)]
    fake: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_line_number(true)
        .compact() // or .pretty()
        .init();

    let args = Args::parse();
    info!(bind=%args.tcp_bind, "can_bridge_daemon starting");

    let shutdown = CancellationToken::new();

    // Ctrl+C watcher cancels the token (don’t block main on ctrl_c directly)
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("Ctrl+C received; cancelling shutdown token");
                shutdown.cancel();
            }
        });
    }

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
    let tcp_server = TcpJsonOrBinaryServer::new(args.tcp_bind.parse()?);
    let tcp_handle = {
        let service = service.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                res = tcp_server.run(service) => {
                    if let Err(e) = res { warn!(error=%e, "tcp server exited with error"); }
                }
                _ = shutdown.cancelled() => {
                    info!("tcp server shutdown requested");
                }
            }
        })
    };

    // WebSocket server
    let ws_server = WsJsonOrBinaryServer::new(args.ws_bind.parse()?);
    let ws_handle = {
        let service = service.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                res = ws_server.run(service) => {
                    if let Err(e) = res { warn!(error=%e, "ws server exited with error"); }
                }
                _ = shutdown.cancelled() => {
                    info!("ws server shutdown requested");
                }
            }
        })
    };

    // gRPC server
    let grpc_server = infra::transport_grpc::GrpcServer::new(args.grpc_bind.parse()?);
    let grpc_handle = {
        let service = service.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                res = grpc_server.run(service) => {
                    if let Err(e) = res { warn!(error=%e, "grpc server exited with error"); }
                }
                _ = shutdown.cancelled() => {
                    info!("grpc server shutdown requested");
                }
            }
        })
    };

    info!("daemon running (terminate with Ctrl+C)");
    // Wait until cancelled
    shutdown.cancelled().await;

    tcp_handle.abort();
    ws_handle.abort();
    grpc_handle.abort();
    info!("shutdown: stopping tasks");
    // These aborts are still useful as a last resort for servers that don’t support graceful stop yet
    tcp_handle.abort();
    ws_handle.abort();
    grpc_handle.abort();

    info!("shutdown complete");

    Ok(())
}
