use anyhow::{anyhow, bail, Result};

pub const MAGIC: [u8; 4] = *b"CBD1";
pub const HEADER_LEN: usize = 12;

// Message exchanged over the binary protocol
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    // client -> daemon
    ClientHello = 101,
    Ping = 102,
    ListIfaces = 103,
    Subscribe = 104,
    Unsubscribe = 105,
    SendFrame = 106,

    // daemon -> client
    HelloAck = 1,
    Pong = 2,
    Ifaces = 3,
    Subscribed = 4,
    Unsubscribed = 5,
    SendAck = 6,
    FrameEvent = 7,
    Error = 8,
}

impl MsgType {
    pub fn from_u16(v: u16) -> Result<Self> {
        Ok(match v {
            101 => Self::ClientHello,
            102 => Self::Ping,
            103 => Self::ListIfaces,
            104 => Self::Subscribe,
            105 => Self::Unsubscribe,
            106 => Self::SendFrame,

            1 => Self::HelloAck,
            2 => Self::Pong,
            3 => Self::Ifaces,
            4 => Self::Subscribed,
            5 => Self::Unsubscribed,
            6 => Self::SendAck,
            7 => Self::FrameEvent,
            8 => Self::Error,
            _ => bail!("unknown msg_type: {v}"),
        })
    }
}

// mirrors JSON Frames
#[derive(Debug, Clone)]
pub enum ClientBinaryRequest {
    ClientHello {
        client: String,
        protocol: String,
    },
    Ping {
        id: u64,
    },
    ListIfaces,
    Subscribe {
        ifaces: Vec<String>,
    },
    Unsubscribe,
    SendFrame {
        iface: String,
        id: u32,
        is_fd: bool,
        brs: bool,
        esi: bool,
        data: Vec<u8>,
    },
}

// mirrors JSON DaemonResponse
#[derive(Debug, Clone)]
pub enum DaemonBinaryResponse {
    HelloAck {
        version: String,
        server_name: String,
        features: Vec<String>,
    },
    Pong {
        id: u64,
    },
    Ifaces {
        items: Vec<String>,
    },
    Subscribed {
        ifaces: Vec<String>,
    },
    Unsubscribed,
    SendAck {
        ok: bool,
        error_message: Option<String>,
    },
    FrameEvent {
        ts_ms: u64,
        iface: String,
        dir: String,
        id: u32,
        is_fd: bool,
        data: Vec<u8>,
    },
    Error {
        message: String,
    },
}

/// Encode a daemon response into one framed packet.
pub fn encode_daemon(resp: &DaemonBinaryResponse) -> Result<Vec<u8>> {
    let (msg_type, payload) = encode_daemon_payload(resp)?;
    encode_packet(msg_type, &payload)
}

/// Encode a client request into one framed packet.
pub fn encode_client(req: &ClientBinaryRequest) -> Result<Vec<u8>> {
    let (msg_type, payload) = encode_client_payload(req)?;
    encode_packet(msg_type, &payload)
}

/// Try to decode a single *client request* from `buffer`.
/// Returns Ok(None) if not enough bytes are available yet.
pub fn decode_client_from(buffer: &[u8]) -> Result<Option<(ClientBinaryRequest, usize)>> {
    let Some((msg_type, payload, consumed_bytes)) = decode_packet(buffer)? else {
        return Ok(None);
    };

    let request = decode_client_payload(msg_type, payload)?;
    Ok(Some((request, consumed_bytes)))
}

/// Try to decode a single *daemon response* from `buffer`.
/// Returns Ok(None) if not enough bytes are available yet.
pub fn decode_daemon_from(buffer: &[u8]) -> Result<Option<(DaemonBinaryResponse, usize)>> {
    let Some((msg_type, payload, consumed_bytes)) = decode_packet(buffer)? else {
        return Ok(None);
    };

    let response = decode_daemon_payload(msg_type, payload)?;
    Ok(Some((response, consumed_bytes)))
}

// ---------------- packet framing ----------------

fn encode_packet(msg_type: MsgType, payload: &[u8]) -> Result<Vec<u8>> {
    let payload_len = payload.len() as u32;
    let mut packet = Vec::with_capacity(HEADER_LEN + payload.len());

    packet.extend_from_slice(&MAGIC);
    packet.extend_from_slice(&(msg_type as u16).to_le_bytes());
    packet.extend_from_slice(&(0u16).to_le_bytes()); // flags reserved
    packet.extend_from_slice(&payload_len.to_le_bytes());
    packet.extend_from_slice(payload);

    Ok(packet)
}

/// Returns:
/// - Ok(None) if incomplete
/// - Ok(Some((msg_type, payload_slice, consumed_bytes)))
fn decode_packet(buffer: &[u8]) -> Result<Option<(MsgType, &[u8], usize)>> {
    if buffer.len() < HEADER_LEN {
        return Ok(None);
    }

    if buffer[0..4] != MAGIC {
        bail!("bad magic");
    }

    let msg_type_raw = u16::from_le_bytes([buffer[4], buffer[5]]);
    let _flags = u16::from_le_bytes([buffer[6], buffer[7]]);
    let payload_len = u32::from_le_bytes([buffer[8], buffer[9], buffer[10], buffer[11]]) as usize;

    let total_len = HEADER_LEN + payload_len;
    if buffer.len() < total_len {
        return Ok(None);
    }

    let msg_type = MsgType::from_u16(msg_type_raw)?;
    let payload = &buffer[HEADER_LEN..total_len];
    Ok(Some((msg_type, payload, total_len)))
}

// ---------------- payload encode: daemon ----------------

fn encode_daemon_payload(resp: &DaemonBinaryResponse) -> Result<(MsgType, Vec<u8>)> {
    let mut payload = Vec::new();

    match resp {
        DaemonBinaryResponse::HelloAck {
            version,
            server_name,
            features,
        } => {
            put_string(&mut payload, version);
            put_string(&mut payload, server_name);

            put_u16(&mut payload, features.len() as u16);
            for f in features {
                put_string(&mut payload, f);
            }
            Ok((MsgType::HelloAck, payload))
        }

        DaemonBinaryResponse::Pong { id } => {
            payload.extend_from_slice(&id.to_le_bytes());
            Ok((MsgType::Pong, payload))
        }

        DaemonBinaryResponse::Ifaces { items } => {
            put_u16(&mut payload, items.len() as u16);
            for iface in items {
                put_string(&mut payload, iface);
            }
            Ok((MsgType::Ifaces, payload))
        }

        DaemonBinaryResponse::Subscribed { ifaces } => {
            put_u16(&mut payload, ifaces.len() as u16);
            for iface in ifaces {
                put_string(&mut payload, iface);
            }
            Ok((MsgType::Subscribed, payload))
        }

        DaemonBinaryResponse::Unsubscribed => Ok((MsgType::Unsubscribed, payload)),

        DaemonBinaryResponse::SendAck { ok, error_message } => {
            payload.push(*ok as u8);
            match error_message {
                Some(msg) => put_string(&mut payload, msg),
                None => put_string(&mut payload, ""),
            }
            Ok((MsgType::SendAck, payload))
        }

        DaemonBinaryResponse::FrameEvent {
            ts_ms,
            iface,
            dir,
            id,
            is_fd,
            data,
        } => {
            payload.extend_from_slice(&ts_ms.to_le_bytes());
            put_string(&mut payload, iface);
            put_string(&mut payload, dir);
            payload.extend_from_slice(&id.to_le_bytes());
            payload.push(*is_fd as u8);
            put_u16(&mut payload, data.len() as u16);
            payload.extend_from_slice(data);
            Ok((MsgType::FrameEvent, payload))
        }

        DaemonBinaryResponse::Error { message } => {
            put_string(&mut payload, message);
            Ok((MsgType::Error, payload))
        }
    }
}

// ---------------- payload decode: daemon ----------------

fn decode_daemon_payload(msg_type: MsgType, mut payload: &[u8]) -> Result<DaemonBinaryResponse> {
    match msg_type {
        MsgType::HelloAck => {
            let version = get_string(&mut payload)?;
            let server_name = get_string(&mut payload)?;
            let features_count = get_u16(&mut payload)? as usize;
            let mut features = Vec::with_capacity(features_count);
            for _ in 0..features_count {
                features.push(get_string(&mut payload)?);
            }

            Ok(DaemonBinaryResponse::HelloAck {
                version,
                server_name,
                features,
            })
        }

        MsgType::Pong => Ok(DaemonBinaryResponse::Pong {
            id: get_u64(&mut payload)?,
        }),

        MsgType::Ifaces => {
            let count = get_u16(&mut payload)? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(get_string(&mut payload)?);
            }
            Ok(DaemonBinaryResponse::Ifaces { items })
        }

        MsgType::Subscribed => {
            let count = get_u16(&mut payload)? as usize;
            let mut ifaces = Vec::with_capacity(count);
            for _ in 0..count {
                ifaces.push(get_string(&mut payload)?);
            }
            Ok(DaemonBinaryResponse::Subscribed { ifaces })
        }

        MsgType::Unsubscribed => Ok(DaemonBinaryResponse::Unsubscribed),

        MsgType::SendAck => {
            let ok = get_u8(&mut payload)? != 0;
            let error_text = get_string(&mut payload)?;
            let error = if error_text.is_empty() {
                None
            } else {
                Some(error_text)
            };
            Ok(DaemonBinaryResponse::SendAck {
                ok,
                error_message: error,
            })
        }

        MsgType::FrameEvent => {
            let ts_ms = get_u64(&mut payload)?;
            let iface = get_string(&mut payload)?;
            let dir = get_string(&mut payload)?;
            let id = get_u32(&mut payload)?;
            let is_fd = get_u8(&mut payload)? != 0;
            let data_len = get_u16(&mut payload)? as usize;

            if payload.len() < data_len {
                bail!("bad data_len");
            }
            let data = payload[..data_len].to_vec();

            Ok(DaemonBinaryResponse::FrameEvent {
                ts_ms,
                iface,
                dir,
                id,
                is_fd,
                data,
            })
        }

        MsgType::Error => Ok(DaemonBinaryResponse::Error {
            message: get_string(&mut payload)?,
        }),

        // Disallow client-only types here
        MsgType::ClientHello
        | MsgType::Ping
        | MsgType::ListIfaces
        | MsgType::Subscribe
        | MsgType::Unsubscribe
        | MsgType::SendFrame => bail!("decode_daemon_payload: got client msg_type: {msg_type:?}"),
    }
}

// ---------------- payload encode: client ----------------

fn encode_client_payload(req: &ClientBinaryRequest) -> Result<(MsgType, Vec<u8>)> {
    let mut payload = Vec::new();

    match req {
        ClientBinaryRequest::ClientHello { client, protocol } => {
            put_string(&mut payload, client);
            put_string(&mut payload, protocol);
            Ok((MsgType::ClientHello, payload))
        }

        ClientBinaryRequest::Ping { id } => {
            payload.extend_from_slice(&id.to_le_bytes());
            Ok((MsgType::Ping, payload))
        }

        ClientBinaryRequest::ListIfaces => Ok((MsgType::ListIfaces, payload)),

        ClientBinaryRequest::Subscribe { ifaces } => {
            put_u16(&mut payload, ifaces.len() as u16);
            for iface in ifaces {
                put_string(&mut payload, iface);
            }
            Ok((MsgType::Subscribe, payload))
        }

        ClientBinaryRequest::Unsubscribe => Ok((MsgType::Unsubscribe, payload)),

        ClientBinaryRequest::SendFrame {
            iface,
            id,
            is_fd,
            brs,
            esi,
            data,
        } => {
            put_string(&mut payload, iface);
            payload.extend_from_slice(&id.to_le_bytes());
            payload.push(*is_fd as u8);
            payload.push(*brs as u8);
            payload.push(*esi as u8);
            put_u16(&mut payload, data.len() as u16);
            payload.extend_from_slice(data);
            Ok((MsgType::SendFrame, payload))
        }
    }
}

// ---------------- payload decode: client ----------------

fn decode_client_payload(msg_type: MsgType, mut payload: &[u8]) -> Result<ClientBinaryRequest> {
    match msg_type {
        MsgType::ClientHello => Ok(ClientBinaryRequest::ClientHello {
            client: get_string(&mut payload)?,
            protocol: get_string(&mut payload)?,
        }),

        MsgType::Ping => Ok(ClientBinaryRequest::Ping {
            id: get_u64(&mut payload)?,
        }),

        MsgType::ListIfaces => Ok(ClientBinaryRequest::ListIfaces),

        MsgType::Subscribe => {
            let count = get_u16(&mut payload)? as usize;
            let mut ifaces = Vec::with_capacity(count);
            for _ in 0..count {
                ifaces.push(get_string(&mut payload)?);
            }
            Ok(ClientBinaryRequest::Subscribe { ifaces })
        }

        MsgType::Unsubscribe => Ok(ClientBinaryRequest::Unsubscribe),

        MsgType::SendFrame => {
            let iface = get_string(&mut payload)?;
            let id = get_u32(&mut payload)?;
            let is_fd = get_u8(&mut payload)? != 0;
            let brs = get_u8(&mut payload)? != 0;
            let esi = get_u8(&mut payload)? != 0;

            let data_len = get_u16(&mut payload)? as usize;
            if payload.len() < data_len {
                bail!("bad data_len");
            }
            let data = payload[..data_len].to_vec();

            Ok(ClientBinaryRequest::SendFrame {
                iface,
                id,
                is_fd,
                brs,
                esi,
                data,
            })
        }

        // Disallow daemon-only types here
        MsgType::HelloAck
        | MsgType::Pong
        | MsgType::Ifaces
        | MsgType::Subscribed
        | MsgType::Unsubscribed
        | MsgType::SendAck
        | MsgType::FrameEvent
        | MsgType::Error => bail!("decode_client_payload: got daemon msg_type: {msg_type:?}"),
    }
}

// ---------------- primitives ----------------

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_string(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(u16::MAX as usize) as u16;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&bytes[..len as usize]);
}

fn get_u8(input: &mut &[u8]) -> Result<u8> {
    if input.len() < 1 {
        bail!("eof");
    }
    let value = input[0];
    *input = &input[1..];
    Ok(value)
}

fn get_u16(input: &mut &[u8]) -> Result<u16> {
    if input.len() < 2 {
        bail!("eof");
    }
    let value = u16::from_le_bytes([input[0], input[1]]);
    *input = &input[2..];
    Ok(value)
}

fn get_u32(input: &mut &[u8]) -> Result<u32> {
    if input.len() < 4 {
        bail!("eof");
    }
    let value = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    *input = &input[4..];
    Ok(value)
}

fn get_u64(input: &mut &[u8]) -> Result<u64> {
    if input.len() < 8 {
        bail!("eof");
    }
    let value = u64::from_le_bytes([
        input[0], input[1], input[2], input[3], input[4], input[5], input[6], input[7],
    ]);
    *input = &input[8..];
    Ok(value)
}

fn get_string(input: &mut &[u8]) -> Result<String> {
    let len = get_u16(input)? as usize;
    if input.len() < len {
        return Err(anyhow!("eof in string"));
    }
    let s = std::str::from_utf8(&input[..len])
        .map_err(|e| anyhow!("utf8: {e}"))?
        .to_string();
    *input = &input[len..];
    Ok(s)
}
