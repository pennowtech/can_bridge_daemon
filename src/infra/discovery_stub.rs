use anyhow::Result;

use crate::ports::discovery::DiscoveryPort;

/// Stub discovery implementation.
/// Replace with netlink-based discovery later.
#[derive(Debug, Clone, Default)]
pub struct StubDiscovery;

impl StubDiscovery {
    pub fn new() -> Self {
        Self
    }
}

impl DiscoveryPort for StubDiscovery {
    fn list_can_ifaces(&self) -> Result<Vec<String>> {
        // Keep stable ordering for tests.
        Ok(vec!["vcan0".to_string(), "can0".to_string()])
    }
}
