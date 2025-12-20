# Rust client SDK skeleton

This is a “thin SDK” that(WIP):

* provides a unified API (`Client`) with methods (`hello`, `ping`, `list_ifaces`, `subscribe`, `send_frame`)
* supports transports:

  * TCP JSONL
  * TCP Binary
  * WS JSONL
  * WS Binary
  * gRPC (optional feature)
* keeps parsing/framing isolated and testable

## Suggested crate layout

```shell
can-bridge-client/
  Cargo.toml
  src/
    lib.rs
    client.rs
    error.rs
    model.rs
    codec/
      mod.rs
      jsonl.rs
      binary.rs
    transport/
      mod.rs
      tcp_jsonl.rs
      tcp_binary.rs
      ws_jsonl.rs
      ws_binary.rs
      grpc.rs        # behind feature "grpc"
  examples/
    ws_jsonl_list_ifaces.rs
    ws_binary_subscribe.rs
    grpc_hello.rs
  tests/
    roundtrip_binary.rs
    roundtrip_jsonl.rs
```

### `Cargo.toml` (skeleton)

```toml
[package]
name = "can-bridge-client"
version = "0.1.0"
edition = "2021"

[features]
default = []
grpc = ["tonic", "prost"]

[dependencies]
thiserror = "1"
bytes = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "time", "sync"] }
futures = "0.3"

serde = { version = "1", features = ["derive"] }
serde_json = "1"

tokio-stream = "0.1"

# WS
tokio-tungstenite = "0.24"
tungstenite = "0.24"
url = "2"

# optional gRPC
tonic = { version = "0.12", optional = true }
prost = { version = "0.13", optional = true }
```

---

## Core types

### `src/model.rs`

```rust
#[derive(Debug, Clone)]
pub struct ClientHello {
    pub client: String,
    pub protocol: String, // "jsonl" | "binary" | "grpc"
}

#[derive(Debug, Clone)]
pub struct HelloAck {
    pub version: Option<String>,
    pub server_name: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub iface: String,
    pub id: u32,
    pub is_fd: bool,
    pub brs: bool,
    pub esi: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FrameEvent {
    pub ts_ms: u64,
    pub iface: String,
    pub dir: Direction,
    pub id: u32,
    pub is_fd: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Rx,
    Tx,
}
```

### `src/error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("ws: {0}")]
    Ws(String),

    #[error("timeout")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, ClientError>;
```

---

## Codec layer (JSONL + Binary)

### `src/codec/jsonl.rs`

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JsonMsg {
    #[serde(rename = "client_hello")]
    ClientHello { client: Option<String>, protocol: Option<String> },

    #[serde(rename = "hello_ack")]
    HelloAck { version: Option<String>, server_name: Option<String>, features: Option<Vec<String>> },

    #[serde(rename = "ping")]
    Ping { id: u64 },

    #[serde(rename = "pong")]
    Pong { id: u64 },

    #[serde(rename = "list_ifaces")]
    ListIfaces {},

    #[serde(rename = "ifaces")]
    Ifaces { items: Vec<String> },

    #[serde(rename = "subscribe")]
    Subscribe { ifaces: Vec<String> },

    #[serde(rename = "subscribed")]
    Subscribed { ifaces: Vec<String> },

    #[serde(rename = "unsubscribe")]
    Unsubscribe {},

    #[serde(rename = "unsubscribed")]
    Unsubscribed {},

    #[serde(rename = "send_frame")]
    SendFrame {
        iface: String,
        id: u32,
        is_fd: bool,
        brs: bool,
        esi: bool,
        data_hex: String,
    },

    #[serde(rename = "send_ack")]
    SendAck { ok: bool, error: Option<String> },

    #[serde(rename = "frame")]
    Frame {
        ts_ms: u64,
        iface: String,
        dir: String,
        id: u32,
        is_fd: bool,
        data_hex: String,
    },

    #[serde(rename = "error")]
    Error { message: String },
}
```

### `src/codec/binary.rs` (framing skeleton)

```rust
use bytes::{Buf, BufMut, BytesMut};
use crate::error::{ClientError, Result};

pub const MAGIC: [u8; 4] = *b"CBD1";
pub const HEADER_LEN: usize = 12;

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub msg_type: u16,
    pub flags: u16,
    pub payload_len: u32,
    pub reserved: u32,
}

impl Header {
    pub fn encode_into(&self, out: &mut BytesMut) {
        out.extend_from_slice(&MAGIC);
        out.put_u16_le(self.msg_type);
        out.put_u16_le(self.flags);
        out.put_u32_le(self.payload_len);
        out.put_u32_le(self.reserved);
    }

    pub fn decode_from(mut buf: &[u8]) -> Result<(Header, usize)> {
        if buf.len() < 4 + HEADER_LEN {
            return Err(ClientError::Protocol("short frame".into()));
        }
        let magic = &buf[..4];
        if magic != MAGIC {
            return Err(ClientError::Protocol("bad magic".into()));
        }
        buf = &buf[4..];

        let msg_type = (&buf[..]).get_u16_le();
        let flags = (&buf[2..]).get_u16_le();
        let payload_len = (&buf[4..]).get_u32_le();
        let reserved = (&buf[8..]).get_u32_le();

        Ok((Header { msg_type, flags, payload_len, reserved }, 4 + HEADER_LEN))
    }
}

// helpers for u16-len-prefixed strings
pub fn put_str(out: &mut BytesMut, s: &str) {
    let b = s.as_bytes();
    out.put_u16_le(b.len() as u16);
    out.extend_from_slice(b);
}

pub fn get_str(buf: &mut &[u8]) -> Result<String> {
    if buf.len() < 2 { return Err(ClientError::Protocol("short str".into())); }
    let len = buf.get_u16_le() as usize;
    if buf.len() < len { return Err(ClientError::Protocol("short str bytes".into())); }
    let s = std::str::from_utf8(&buf[..len]).map_err(|_| ClientError::Protocol("utf8".into()))?;
    *buf = &buf[len..];
    Ok(s.to_string())
}
```

---

## Transport layer traits

### `src/transport/mod.rs`

```rust
use crate::error::Result;

#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    async fn send(&mut self, bytes: &[u8]) -> Result<()>;
    async fn recv(&mut self) -> Result<Vec<u8>>;
}
```

(Use `async-trait` if you want; or make it concrete per transport.)

---

## High-level client

### `src/client.rs`

```rust
use crate::{error::Result, model::*};

pub struct Client {
    inner: Box<dyn ClientImpl>,
}

#[async_trait::async_trait]
pub trait ClientImpl: Send + Sync {
    async fn hello(&mut self, h: ClientHello) -> Result<HelloAck>;
    async fn ping(&mut self, id: u64) -> Result<u64>;
    async fn list_ifaces(&mut self) -> Result<Vec<String>>;
    async fn send_frame(&mut self, frame: Frame) -> Result<()>;
    // subscribe returns a stream in real SDK; keep skeleton simple
}

impl Client {
    pub fn new(inner: Box<dyn ClientImpl>) -> Self { Self { inner } }

    pub async fn hello(&mut self, h: ClientHello) -> Result<HelloAck> { self.inner.hello(h).await }
    pub async fn ping(&mut self, id: u64) -> Result<u64> { self.inner.ping(id).await }
    pub async fn list_ifaces(&mut self) -> Result<Vec<String>> { self.inner.list_ifaces().await }
    pub async fn send_frame(&mut self, frame: Frame) -> Result<()> { self.inner.send_frame(frame).await }
}
```

### Implementations

Create concrete clients:

* `WsJsonlClient`
* `WsBinaryClient`
* `TcpJsonlClient`
* `TcpBinaryClient`
* `GrpcClient` (feature-gated)

Each one:

* establishes connection
* performs the **client-first handshake** for TCP/WS
* encodes/decodes using appropriate codec

This keeps our daemon’s “one core, multiple transports” concept mirrored client-side.
