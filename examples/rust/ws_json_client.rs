// SPDX-License-Identifier: Apache-2.0
//! ws_json_client
//!
//! Layer: Unknown
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<()> {
    let url = "ws://127.0.0.1:29537/ws";
    let iface = "can0";

    let (mut ws, _) = tokio_tungstenite::connect_async(url).await?;

    ws.send(Message::Text(json!({
        "type":"ClientHello",
        "client":"rust-ws-json",
        "client_version":"0.1.0",
        "features":["ws-json"]
    }).to_string())).await?;

    let msg = ws.next().await.ok_or_else(|| anyhow::anyhow!("closed"))??;
    let v: serde_json::Value = serde_json::from_str(msg.to_text()?)?;
    if v.get("type") != Some(&json!("HelloAck")) {
        bail!("expected HelloAck, got {v}");
    }
    println!("HelloAck: {v}");

    ws.send(Message::Text(json!({"type":"Subscribe","iface":iface}).to_string())).await?;
    println!("Subscribed requested.");  

    while let Some(m) = ws.next().await {
        let m = m?;
        if let Message::Text(t) = m {
            println!("recv: {t}");
        }
    }
    Ok(())
}
     