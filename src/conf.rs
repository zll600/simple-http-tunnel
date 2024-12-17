use std::time::Instant;

use regex::Regex;
use serde::{self, Deserialize};
use tokio::time::Duration;

#[derive(Clone)]
pub struct ProxyConfiguration {
    pub bind_address: String,
    pub tunnel_config: TunnelConfig,
}

#[derive(Clone)]
pub struct TunnelConfig {
    pub client_connection: ClientConnectionConfig,
    pub target_connection: TargetConnectionConfig,
}

#[derive(Deserialize, Clone)]
pub struct ClientConnectionConfig {
    // #[serde(with = "humantime_serde")]
    // pub dns_cache_ttl: Duration,
    // #[serde(with = "serde_regex")]
    // pub allowed_targets: Regex,
    // #[serde(with = "humantime_serde")]
    // pub connect_timeout: Duration,
    // pub relay_policy: RelayPolicy,
    pub connect_timeout: Duration,
}

#[derive(Deserialize, Clone)]
pub struct TargetConnectionConfig {
    // #[serde(with = "humantime_serde")]
    // pub dns_cache_ttl: Duration,
    // #[serde(with = "serde_regex")]
    // pub allowed_targets: Regex,
    // #[serde(with = "humantime_serde")]
    // pub connect_timeout: Duration,
    // pub relay_policy: RelayPolicy,
    pub connect_timeout: Duration,
}

#[derive(Deserialize, Clone)]
pub struct RelayPolicy {
    #[serde(with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// Min bytes-per-minute (bpm)
    pub min_rate_bpm: u64,
    // Max bytes-per-second (bps)
    pub max_rate_bps: u64,
}
