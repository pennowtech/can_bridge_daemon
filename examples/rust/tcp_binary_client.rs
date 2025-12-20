// SPDX-License-Identifier: Apache-2.0
//! tcp_binary_client
//!
//! Layer: Unknown
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

use anyhow::{bail, Result};
use bytes::{BufMut, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAGIC: &[u8; 4] = b"CBD1";
const HEADER_LEN: usize = 8; // msg_type u16 + flags u16 + payload_len u32

// MsgTypes (spec)
const HELLO_ACK: u16 = 1;
const IFACES: u16 = 3;
const SUBSCRIBED: u16 = 4;
const FRAME_EVENT: u16 = 7;
const ERROR: u16 = 8;

const CLIENT_HELLO: u16 = 101;
const LIST_IFACES: u16 = 103;
const SUBSCRIBE: u16 = 104;

fn put_u16_be(buf: &mut BytesMut, v: u16) { buf.put_u16(v); }
fn put_u32_be(buf: &mut BytesMut, v: u32) { buf.put_u32(v); }

fn pack_str(s: &str) -> BytesMut {
    let mut b = BytesMut::new();
    put_u16_be(&mut b, s.len() as u16);
    b.extend_from_slice(s.as_bytes());
    b
}

fn encode_frame(msg_type: u16, flags: u16, payload: &[u8]) -> BytesMut {
    let mut out = BytesMut::with_capacity(4 + HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    put_u16_be(&mut out, msg_type);
    put_u16_be(&mut out, flags);
    put_u32_be(&mut out, payload.len() as u32);
    out.extend_from_slice(payload);
    out
}

async fn read_exact(stream: &mut TcpStream, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn read_msg(stream: &mut TcpStream) -> Result<(u16, u16, Vec<u8>)> {
    let magic = read_exact(stream, 4).await?;
    if magic.as_slice() != MAGIC {
        bail!("bad magic: {:?}", magic);
    }
    let hdr = read_exact(stream, HEADER_LEN).await?;
    let msg_type = u16::from_be_bytes([hdr[0], hdr[1]]);
    let flags = u16::from_be_bytes([hdr[2], hdr[3]]);
    let len = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    let payload = if len > 0 { read_exact(stream, len).await? } else { vec![] };
    Ok((msg_type, flags, payload))
}

#[tokio::main]
async fn main() -> Result<()> {
    let addr = "127.0.0.1:29536";
    let iface = "can0";

    let mut s = TcpStream::connect(addr).await?;

    // hello
    let mut payload = BytesMut::new();
    payload.extend_from_slice(&pack_str("rust-tcp-binary"));
    payload.extend_from_slice(&pack_str("0.1.0"));
    let frame = encode_frame(CLIENT_HELLO, 0, &payload);
    s.write_all(&frame).await?;

    let (mt, _, p) = read_msg(&mut s).await?;
    if mt == ERROR { bail!("server error: {:?}", p); }
    if mt != HELLO_ACK { bail!("expected HelloAck, got {mt}"); }
    println!("HelloAck ok");

    // list ifaces
    let frame = encode_frame(LIST_IFACES, 0, &[]);
    s.write_all(&frame).await?;
    let (mt, _, p) = read_msg(&mut s).await?;
    if mt != IFACES { bail!("expected Ifaces, got {mt} payload_len={}", p.len()); }
    println!("Ifaces payload len={}", p.len());

    // subscribe
    let payload = pack_str(iface);
    let frame = encode_frame(SUBSCRIBE, 0, &payload);
    s.write_all(&frame).await?;
    let (mt, _, _) = read_msg(&mut s).await?;
    if mt != SUBSCRIBED { bail!("expected Subscribed, got {mt}"); }
    println!("Subscribed {iface}");

    // stream
    loop {
        let (mt, _, p) = read_msg(&mut s).await?;
        if mt == FRAME_EVENT {
            println!("FrameEvent payload len={}", p.len());
        } else if mt == ERROR {
            println!("Error payload len={}", p.len());
        } else {
            println!("Other msg_type={mt} payload_len={}", p.len());
        }
    }
}
