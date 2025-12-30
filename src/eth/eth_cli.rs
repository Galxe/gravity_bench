use alloy::{
    consensus::{Account, TxEnvelope},
    eips::Encodable2718,
    network::Ethereum,
    primitives::{Address, TxHash, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::types::TransactionReceipt,
};
use anyhow::{Context as AnyhowContext, Result};
use comfy_table::{presets::UTF8_FULL, Cell, Table};
use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};
use url::Url;

/// Format large numbers with appropriate suffixes (K, M, B)
fn format_large_number(num: u64) -> String {
    if num >= 1_000_000_000 {
        format!("{:.1}B", num as f64 / 1_000_000_000.0)
    } else if num >= 1_000_000 {
        format!("{:.1}M", num as f64 / 1_000_000.0)
    } else if num >= 10_000 {
        format!("{:.1}K", num as f64 / 1_000.0)
    } else {
        num.to_string()
    }
}

#[derive(Debug, Default, Clone)]
pub struct MethodMetrics {
    pub requests_sent: u64,
    pub requests_succeeded: u64,
    pub requests_failed: u64,
    pub total_latency_ms: u64,
}

#[derive(Debug, Default, Clone)]
pub struct ProviderMetrics {
    pub per_method: HashMap<String, MethodMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MempoolStatus {
    #[serde(deserialize_with = "deserialize_hex_to_usize")]
    pub pending: usize,
    #[serde(deserialize_with = "deserialize_hex_to_usize")]
    pub queued: usize,
}

fn deserialize_hex_to_usize<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    if s.starts_with("0x") {
        usize::from_str_radix(&s[2..], 16).map_err(serde::de::Error::custom)
    } else {
        s.parse::<usize>().map_err(serde::de::Error::custom)
    }
}

/// Ethereum transaction sender, providing reliable communication with nodes
#[derive(Clone)]
pub struct EthHttpCli {
    inner: Vec<Arc<RootProvider<Ethereum>>>,
    #[allow(unused)]
    chain_id: u64,
    metrics: Arc<tokio::sync::Mutex<ProviderMetrics>>,
    retry_config: RetryConfig,
    rpc: Arc<String>,
}

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        }
    }
}

impl EthHttpCli {
    pub fn rpc(&self) -> Arc<String> {
        self.rpc.clone()
    }

    /// Create new TxnSender instance
    pub fn new(rpc_url: &str, chain_id: u64) -> Result<Self> {
        debug!(
            "Creating TxnSender for URL: {}, Chain ID: {}",
            rpc_url, chain_id
        );
        // Parse URL

        let url =
            Url::parse(rpc_url).with_context(|| format!("Failed to parse RPC URL: {}", rpc_url))?;
        let mut inner = Vec::new();
        for _ in 0..1 {
            // let client = reqwest::Client::builder()
            //     // .pool_idle_timeout(Duration::from_secs(120))
            //     // .pool_max_idle_per_host(2000)
            //     // .connect_timeout(Duration::from_secs(10))
            //     // .timeout(Duration::from_secs(5))
            //     // .tcp_keepalive(Duration::from_secs(30))
            //     // .tcp_nodelay(true)
            //     // .http2_prior_knowledge()
            //     // .http2_adaptive_window(true)
            //     // .http2_keep_alive_timeout(Duration::from_secs(10))
            //     // .no_gzip()
            //     // .no_brotli()
            //     // .no_deflate()
            //     // .no_zstd()
            //     .build()
            //     .unwrap();

            let provider: RootProvider<Ethereum> =
                ProviderBuilder::default().connect_http(url.clone());

            inner.push(Arc::new(provider));
        }

        let txn_sender = Self {
            rpc: Arc::new(rpc_url.to_string()),
            inner,
            chain_id,
            metrics: Arc::new(tokio::sync::Mutex::new(ProviderMetrics::default())),
            retry_config: RetryConfig::default(),
        };

        // Verify connection

        debug!("TxnSender created successfully");
        Ok(txn_sender)
    }

    #[allow(unused)]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub async fn get_txn_count(&self, address: Address) -> Result<u64> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let nonce = self.inner[0].get_transaction_count(address).await?;
            Ok(nonce)
        })
        .await?
    }

    /// Verify network connection
    #[allow(unused)]
    async fn verify_connection(&self) -> Result<()> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async {
                let _block_number = self.inner[0].get_block_number().await?;
                Ok(())
            })
            .await;

        self.update_metrics("eth_blockNumber", result.is_ok(), start.elapsed())
            .await;

        result.with_context(|| "Failed to verify connection to Ethereum node")
    }

    /// Get account transaction count (nonce)
    #[allow(unused)]
    pub async fn get_transaction_count(&self, address: Address) -> Result<u64> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async { self.inner[0].get_transaction_count(address).await })
            .await;

        self.update_metrics("eth_getTransactionCount", result.is_ok(), start.elapsed())
            .await;

        result
            .with_context(|| format!("Failed to get transaction count for address: {:?}", address))
    }

    /// Get account balance
    #[allow(unused)]
    pub async fn get_balance(&self, address: &Address) -> Result<U256> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async { self.inner[0].get_balance(*address).await })
            .await;

        self.update_metrics("eth_getBalance", result.is_ok(), start.elapsed())
            .await;

        result.with_context(|| format!("Failed to get balance for address: {:?}", address))
    }

    /// Get current gas price
    #[allow(unused)]
    pub async fn get_gas_price(&self) -> Result<u128> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async { self.inner[0].get_gas_price().await })
            .await;

        self.update_metrics("eth_gasPrice", result.is_ok(), start.elapsed())
            .await;

        result
            .map_err(|e| anyhow::anyhow!("Failed to get gas price: {:?}", e))
            .with_context(|| "Failed to get gas price")
    }

    /// Get mempool status
    pub async fn get_mempool_status(&self) -> Result<MempoolStatus> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async {
                let result: MempoolStatus = self.inner[0]
                    .raw_request::<(), MempoolStatus>("txpool_status".into(), ())
                    .await?;
                Ok(result)
            })
            .await;

        self.update_metrics("txpool_status", result.is_ok(), start.elapsed())
            .await;

        result.with_context(|| "Failed to get mempool status")
    }

    /// Get latest block number
    #[allow(unused)]
    pub async fn get_block_number(&self) -> Result<u64> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async { self.inner[0].get_block_number().await })
            .await;

        self.update_metrics("eth_blockNumber", result.is_ok(), start.elapsed())
            .await;

        result.with_context(|| "Failed to get block number")
    }

    /// Execute operation with retry mechanism
    async fn retry_with_backoff<F, Fut, T>(&self, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, alloy::transports::TransportError>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.retry_config.max_retries {
            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        debug!("Operation succeeded on attempt {}", attempt + 1);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.retry_config.max_retries {
                        let delay = std::cmp::min(
                            Duration::from_millis(
                                (self.retry_config.base_delay.as_millis() as f64
                                    * self.retry_config.backoff_multiplier.powi(attempt as i32))
                                    as u64,
                            ),
                            self.retry_config.max_delay,
                        );
                        warn!(
                            "Operation failed on attempt {}, retrying in {:?}: {:?}",
                            attempt + 1,
                            delay,
                            last_error
                        );
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(anyhow::anyhow!(
            "Operation failed after {} attempts. Last error: {:?}",
            self.retry_config.max_retries + 1,
            last_error
        ))
    }

    /// Update performance metrics
    async fn update_metrics(&self, method: &str, success: bool, latency: Duration) {
        let mut metrics = self.metrics.lock().await;
        let method_metrics = metrics.per_method.entry(method.to_string()).or_default();

        method_metrics.requests_sent += 1;

        if success {
            method_metrics.requests_succeeded += 1;
        } else {
            method_metrics.requests_failed += 1;
        }

        // Ensure at least 1ms latency is recorded to avoid 0 latency in very fast environments
        let latency_ms = std::cmp::max(1, latency.as_millis() as u64);
        method_metrics.total_latency_ms += latency_ms;
    }

    /// Get a copy of performance metrics
    #[allow(unused)]
    pub async fn get_metrics(&self) -> ProviderMetrics {
        self.metrics.lock().await.clone()
    }

    /// Log performance metrics
    #[allow(unused)]
    pub async fn log_metrics_summary(&self) {
        let metrics = self.get_metrics().await;
        if metrics.per_method.is_empty() {
            info!("RPC Metrics for [{}]: No requests recorded yet.", self.rpc);
            return;
        }

        let mut table = Table::new();
        table.load_preset(UTF8_FULL);

        // Set proper column headers for RPC metrics
        table.set_header(vec![
            "RPC Method",
            "Sent",
            "Succeeded",
            "Failed",
            "Success Rate",
            "Avg Latency",
        ]);

        // Add data rows
        for (method, stats) in &metrics.per_method {
            let success_rate = if stats.requests_sent > 0 {
                stats.requests_succeeded as f64 / stats.requests_sent as f64 * 100.0
            } else {
                0.0
            };
            let avg_latency = if stats.requests_sent > 0 {
                stats.total_latency_ms as f64 / stats.requests_sent as f64
            } else {
                0.0
            };

            table.add_row(vec![
                Cell::new(method),
                Cell::new(&format_large_number(stats.requests_sent)),
                Cell::new(&format_large_number(stats.requests_succeeded)),
                Cell::new(&format_large_number(stats.requests_failed)),
                Cell::new(&format!("{:.1}%", success_rate)),
                Cell::new(&format!("{:.1}ms", avg_latency)),
            ]);
        }

        // Add summary row for RPC metrics
        let total_sent: u64 = metrics.per_method.values().map(|m| m.requests_sent).sum();
        let total_succeeded: u64 = metrics
            .per_method
            .values()
            .map(|m| m.requests_succeeded)
            .sum();
        let total_failed: u64 = metrics.per_method.values().map(|m| m.requests_failed).sum();
        let overall_success_rate = if total_sent > 0 {
            total_succeeded as f64 / total_sent as f64 * 100.0
        } else {
            0.0
        };
        let overall_avg_latency = if total_sent > 0 {
            let total_latency: u64 = metrics
                .per_method
                .values()
                .map(|m| m.total_latency_ms)
                .sum();
            total_latency as f64 / total_sent as f64
        } else {
            0.0
        };

        table.add_row(vec![
            Cell::new("TOTAL"),
            Cell::new(&format_large_number(total_sent)),
            Cell::new(&format_large_number(total_succeeded)),
            Cell::new(&format_large_number(total_failed)),
            Cell::new(&format!("{:.1}%", overall_success_rate)),
            Cell::new(&format!("{:.1}ms", overall_avg_latency)),
        ]);

        info!("\n{}", table);
    }

    /// Reset metrics
    #[allow(unused)]
    pub async fn reset_metrics(&self) {
        let mut metrics = self.metrics.lock().await;
        *metrics = ProviderMetrics::default();
        debug!("TxnSender metrics reset");
    }

    pub async fn send_raw_tx(&self, tx_bytes: Vec<u8>) -> Result<TxHash> {
        let idx = rand::thread_rng().gen_range(0..self.inner.len());
        let start = Instant::now();
        let op = async {
            let pending_tx = self.inner[idx].send_raw_transaction(&tx_bytes).await?;
            anyhow::Ok(pending_tx.tx_hash().clone())
        };

        let result = tokio::time::timeout(Duration::from_secs(10), op).await;

        let final_result = match result {
            Ok(Ok(hash)) => Ok(hash.clone()),
            Ok(Err(e)) => Err(anyhow::Error::from(e)),
            Err(e) => Err(anyhow::Error::from(e)),
        };

        self.update_metrics(
            "eth_sendRawTransaction",
            final_result.is_ok(),
            start.elapsed(),
        )
        .await;

        final_result
    }

    /// Send signed transaction envelope
    #[allow(unused)]
    pub async fn send_tx_envelope(&self, tx_envelope: TxEnvelope) -> Result<TxHash> {
        let start = Instant::now();
        let idx = rand::thread_rng().gen_range(0..self.inner.len());
        let result = self
            .retry_with_backoff(|| async {
                let start = Instant::now();
                let encoded_tx = tx_envelope.encoded_2718();
                let pending_tx = self.inner[idx].send_raw_transaction(&encoded_tx).await?;
                let latency = start.elapsed();
                if rand::thread_rng().gen_bool(0.0001) {
                    println!("send_tx_envelope latency: {:?}", latency);
                }
                Ok(*pending_tx.tx_hash())
            })
            .await;

        self.update_metrics("eth_sendRawTransaction", result.is_ok(), start.elapsed())
            .await;

        result.with_context(|| "Failed to send transaction envelope")
    }

    /// Wait for transaction confirmation and get receipt
    pub async fn get_transaction_receipt(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<TransactionReceipt>> {
        let idx = rand::thread_rng().gen_range(0..self.inner.len());
        let start = Instant::now();
        let result = self
            .retry_with_backoff(|| async { self.inner[idx].get_transaction_receipt(tx_hash).await })
            .await;

        self.update_metrics("eth_getTransactionReceipt", result.is_ok(), start.elapsed())
            .await;

        result.with_context(|| format!("Failed to get transaction receipt for hash: {:?}", tx_hash))
    }

    pub async fn get_account(&self, address: Address) -> Result<Account> {
        self.retry_with_backoff(|| async { self.inner[0].get_account(address).await })
            .await
    }
}
