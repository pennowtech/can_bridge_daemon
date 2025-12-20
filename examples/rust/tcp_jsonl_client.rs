// SPDX-License-Identifier: Apache-2.0
//! tcp_jsonl_client
//!
//! Layer: Unknown
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

use anyhow::{bail, Result};
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

#[tokio::main]
async fn main() -> Result<()> {
    let host = "127.0.0.1:29535";
    let iface = "can0";

    let stream = TcpStream::connect(host).await?;
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);

    async fn send(w: &mut tokio::net::tcp::OwnedWriteHalf, v: serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_vec(&v)?;
        line.push(b'\n');
        w.write_all(&line).await?;
        Ok(())
    }

    async fn recv(r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Result<serde_json::Value> {
        let mut line = String::new();
        let n = r.read_line(&mut line).await?;
        if n == 0 {
            bail!("connection closed");
        }
        Ok(serde_json::from_str(&line)?)
    }

    // hello
    send(&mut w, json!({
        "type": "ClientHello",
        "client": "rust-tcp-jsonl",
        "client_version": "0.1.0",
        "features": ["jsonl"]
    }))
    .await?;
    let msg = recv(&mut r).await?;
    if msg.get("type") != Some(&json!("HelloAck")) {
        bail!("expected HelloAck, got {msg}");
    }
    println!("HelloAck: {msg}");

    // list ifaces
    send(&mut w, json!({"type":"ListIfaces"})).await?;
    let msg = recv(&mut r).await?;
    println!("Ifaces: {msg}");

    // subscribe
    send(&mut w, json!({"type":"Subscribe","iface":iface})).await?;
    let msg = recv(&mut r).await?;
    println!("Subscribed: {msg}");

    // send frame (optional)
    send(&mut w, json!({
        "type":"SendFrame",
        "iface": iface,
        "frame": {"id":291,"extended":true,"fd":false,"brs":false,"data":"11223344"}
    }))
    .await?;
    let msg = recv(&mut r).await?;
    println!("SendFrame response: {msg}");

    // stream
    loop {
        let msg = recv(&mut r).await?;
        println!("recv: {msg}");
    }
}
