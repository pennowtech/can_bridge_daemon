use serde::{Deserialize, Serialize};

/// Requests sent *to* the daemon (from clients).
///
/// We use serde's "tag" representation:
/// { "type": "ping", "id": 1 }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    /// handshake ack sent by client after receiving ServerResponse::Hello
    HelloAck { client: String, protocol: String },

    /// Simple liveness test
    Ping { id: u64 },

    /// ask daemon for available CAN interfaces (stub for now)
    ListIfaces,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerResponse {
    /// handshake greeting sent immediately on connect
    Hello {
        version: String,
        features: Vec<String>,
        server_name: String,
    },

    /// Ping response
    Pong { id: u64 },

    /// list of interfaces
    Ifaces { items: Vec<String> },

    /// Generic error response (protocol, parsing, etc.)
    Error { message: String },
}
