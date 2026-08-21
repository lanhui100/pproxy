pub mod source;
pub mod check;
pub mod pool;
pub mod relay;
pub mod edge;

pub use pool::Pool;
pub use edge::{EdgeClient, ForwardRequest};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ProxyRecord {
    pub ip: String,
    pub port: u16,
    pub protocol: String,
    pub country: Option<String>,
    pub anonymity: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_verified: Option<u64>,
}

impl ProxyRecord {
    pub fn addr(&self) -> String {
        format!("{}:{}", self.ip, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub listen_host: String,
    pub listen_port: u16,
    #[serde(default)]
    pub static_upstreams: Vec<String>,
    #[serde(default)]
    pub countries: Vec<String>,
    #[serde(default = "default_pool_refresh_sec")]
    pub pool_refresh_sec: u64,
    #[serde(default = "default_pool_ttl_sec")]
    pub pool_ttl_sec: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
    #[serde(default = "default_verify_concurrency")]
    pub verify_concurrency: usize,
    #[serde(default)]
    pub worker_url: Option<String>,
    #[serde(default)]
    pub worker_secret: Option<String>,
    #[serde(default)]
    pub routes: HashMap<String, String>,
    #[serde(default)]
    pub upstreams: HashMap<String, UpstreamConfig>,
    #[serde(default)]
    pub route_upstreams: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub url: String,
    pub secret: String,
}

fn default_pool_refresh_sec() -> u64 {
    300
}
fn default_pool_ttl_sec() -> u64 {
    180
}
fn default_max_retries() -> usize {
    3
}
fn default_verify_concurrency() -> usize {
    30
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            listen_host: "127.0.0.1".into(),
            listen_port: 8899,
            static_upstreams: vec![],
            countries: vec![],
            pool_refresh_sec: 300,
            pool_ttl_sec: 180,
            max_retries: 3,
            verify_concurrency: 30,
            worker_url: None,
            worker_secret: None,
            routes: HashMap::new(),
            upstreams: HashMap::new(),
            route_upstreams: HashMap::new(),
        }
    }
}
