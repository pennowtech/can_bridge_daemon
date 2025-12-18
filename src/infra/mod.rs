// SPDX-License-Identifier: Apache-2.0
//! mod
//!
//! Layer: Infrastructure
//! Purpose:
//! - TODO: describe this module briefly
//!
//! Notes:
//! - Standard file header. Keep stable to avoid churn.

pub mod fake_generator;
pub mod iface_discovery_dummy;
pub mod iface_discovery_netlink;
pub mod socketcan_rx;
pub mod socketcan_tx;
pub mod transport_grpc;
pub mod transport_tcp;
pub mod transport_ws;
