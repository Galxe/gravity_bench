use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Complete configuration structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BenchConfig {
    pub nodes: Vec<NodeConfig>,
    pub faucet: FaucetConfig,
    pub accounts: AccountConfig,
    pub performance: PerformanceConfig,
    pub contract_config_path: String,
    pub num_tokens: usize,
    pub target_tps: u64,
    pub enable_swap_token: bool,
}

/// Node and chain configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NodeConfig {
    pub rpc_url: String,
    pub chain_id: u64,
}

/// Faucet and deployer account configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaucetConfig {
    pub private_key: String,
    pub faucet_level: u64,
    pub init_token_balance: String,
}

/// Load testing account configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AccountConfig {
    pub num_accounts: usize,
}

/// Performance and stress configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PerformanceConfig {
    /// Number of concurrent transaction sending tasks inside TxnConsumer
    pub num_senders: usize,
    /// Maximum capacity of the transaction pool inside Consumer
    pub max_pool_size: usize,
}

impl BenchConfig {
    /// Load configuration from TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;

        let config: BenchConfig =
            toml::from_str(&content).with_context(|| "Failed to parse config file as TOML")?;

        Ok(config)
    }
}
