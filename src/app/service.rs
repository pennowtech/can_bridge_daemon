use std::sync::Arc;

use tokio::sync::broadcast;

use crate::{
    domain::{
        frame::FrameEvent,
        protocol::{ClientRequest, ServerResponse},
    },
    ports::discovery::DiscoveryPort,
};

/// BridgeService = application layer entry point.
/// In later steps this will coordinate:
/// - discovery port
/// - can rx/tx ports
/// - subscriptions
/// - replay sources
#[derive(Clone)]
pub struct BridgeService {
    discovery: Arc<dyn DiscoveryPort>,
    frames_tx: broadcast::Sender<FrameEvent>,
}

impl std::fmt::Debug for BridgeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeService").finish()
    }
}

impl BridgeService {
    /// Create a new service with a broadcast bus for frames.
    pub fn new(discovery: Arc<dyn DiscoveryPort>) -> Self {
        // Capacity chosen for dev; tune later. If receivers lag, they’ll drop messages.
        let (frames_tx, _) = broadcast::channel::<FrameEvent>(1024);
        Self {
            discovery,
            frames_tx,
        }
    }

    /// Subscribe to the internal frame bus.
    /// Each TCP connection gets its own receiver.
    pub fn subscribe_frames(&self) -> broadcast::Receiver<FrameEvent> {
        self.frames_tx.subscribe()
    }

    /// Publish a frame event into the system (used by fake generator now,
    /// and later by SocketCAN RX and replay sources).
    pub fn publish_frame(&self, ev: FrameEvent) {
        // Ignore send errors (only happens if no receivers).
        let _ = self.frames_tx.send(ev);
    }

    /// Handle a single request and return a response.
    /// NOTE: Subscribe/Unsubscribe are handled in the transport layer because they
    /// are per-connection state. (Transport replies with Subscribed/Unsubscribed.)
    pub async fn handle(&self, req: ClientRequest) -> ServerResponse {
        match req {
            ClientRequest::Ping { id } => ServerResponse::Pong { id },
            ClientRequest::ListIfaces => match self.discovery.list_can_ifaces().await {
                Ok(items) => ServerResponse::Ifaces { items },
                Err(e) => ServerResponse::Error {
                    message: format!("list_ifaces failed: {e}"),
                },
            },

            ClientRequest::HelloAck { .. } => ServerResponse::Error {
                message: "hello_ack is handled by transport layer".to_string(),
            },

            ClientRequest::Subscribe { .. } | ClientRequest::Unsubscribe => ServerResponse::Error {
                message: "subscribe/unsubscribe are handled by transport layer".to_string(),
            },
        }
    }
}
