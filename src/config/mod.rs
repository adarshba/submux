use serde::{Deserialize, Serialize};

use crate::router::{FallbackChain, StrategyKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmuxConfig {
    pub server: ServerConfig,
    pub routing: RoutingConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub model_groups: Vec<ModelGroupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
    pub max_concurrent_per_account: usize,
    pub upstream_timeout_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_owned(),
            max_concurrent_per_account: 4,
            upstream_timeout_secs: 900,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    pub strategy: StrategyKind,
    pub max_account_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum StorageConfig {
    Memory,
    Sqlite { path: String },
    Postgres { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelGroupConfig {
    pub name: String,
    pub accounts: Vec<String>,
    #[serde(default)]
    pub fallbacks: FallbackChain,
}
