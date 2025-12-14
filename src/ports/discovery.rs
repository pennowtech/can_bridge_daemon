use anyhow::Result;

/// DiscoveryPort = outbound port (application depends on this).
/// Infrastructure provides implementations (stub now, netlink later).
pub trait DiscoveryPort: Send + Sync {
    fn list_can_ifaces(&self) -> Result<Vec<String>>;
}
