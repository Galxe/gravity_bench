use alloy::primitives::TxHash;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

use crate::eth::EthHttpCli;

/// Provider dispatcher trait for routing transactions to different providers
#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Send a transaction envelope to a provider selected by the dispatcher
    /// Returns (TxHash, RPC URL) on success
    async fn send_tx(
        &self,
        tx_bytes: Vec<u8>,
        txn_id: Uuid,
    ) -> std::result::Result<(TxHash, String), (anyhow::Error, String)>;

    async fn provider(&self, rpc: &str) -> Result<Arc<EthHttpCli>>;
}

/// Simple dispatcher that routes transactions based on txn_id modulo
#[derive(Clone)]
pub struct SimpleDispatcher {
    providers: Vec<Arc<EthHttpCli>>,
}

impl SimpleDispatcher {
    pub fn new(providers: Vec<EthHttpCli>) -> Self {
        let providers = providers.into_iter().map(Arc::new).collect();
        Self { providers }
    }

    /// Select provider based on txn_id hash
    fn select_provider(&self, txn_id: Uuid) -> &Arc<EthHttpCli> {
        let hash = txn_id.as_u128();
        let index = (hash % self.providers.len() as u128) as usize;
        &self.providers[index]
    }
}

#[async_trait]
impl Dispatcher for SimpleDispatcher {
    async fn send_tx(
        &self,
        bytes: Vec<u8>,
        txn_id: Uuid,
    ) -> std::result::Result<(TxHash, String), (anyhow::Error, String)> {
        let provider = self.select_provider(txn_id);
        let rpc_url = provider.rpc().as_ref().clone();
        let tx_hash = provider
            .send_raw_tx(bytes)
            .await
            .map_err(|e| (e, provider.rpc().as_ref().clone()))?;

        Ok((tx_hash, rpc_url))
    }

    async fn provider(&self, rpc: &str) -> Result<Arc<EthHttpCli>> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.rpc().as_str() == rpc)
            .ok_or(anyhow::anyhow!("Provider not found"))?;
        Ok(provider.clone())
    }
}
