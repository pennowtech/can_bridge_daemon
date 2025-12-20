// SPDX-License-Identifier: Apache-2.0
//! build
//!
//! Layer: Unknown
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile(&["proto/can_bridge.proto"], &["proto"])?;
    Ok(())
}
