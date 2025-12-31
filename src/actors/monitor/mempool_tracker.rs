use std::{collections::HashMap, sync::Arc};

use actix::Addr;

use crate::{
    actors::{PauseProducer, Producer, ResumeProducer},
    eth::{EthHttpCli, MempoolStatus, TxPoolContent},
};

use super::NonceCorrectionInfo;

/// Action to take after analyzing mempool status
#[derive(Debug)]
pub enum MempoolAction {
    /// No action needed
    None,
    /// Pause producer due to mempool being full
    Pause,
    /// Resume producer
    Resume,
    /// Need to fetch txpool_content for nonce correction
    NeedsNonceCorrection,
}

pub(crate) struct MempoolTracker {
    max_pool_size: usize,
    /// Threshold for queued/pending ratio to trigger nonce correction
    queued_ratio_threshold: f64,
    /// Cooldown to avoid frequent txpool_content calls
    last_correction_check: std::time::Instant,
    correction_cooldown: std::time::Duration,
}

impl MempoolTracker {
    pub fn new(max_pool_size: usize) -> Self {
        Self {
            max_pool_size,
            queued_ratio_threshold: 1.0,
            last_correction_check: std::time::Instant::now(),
            correction_cooldown: std::time::Duration::from_secs(30),
        }
    }

    pub async fn get_pool_status(
        clients: &HashMap<Arc<String>, Arc<EthHttpCli>>,
    ) -> Vec<anyhow::Result<MempoolStatus>> {
        let mut statuses = Vec::new();
        for client in clients.values() {
            let pool_status = client.get_mempool_status().await;
            statuses.push(pool_status);
        }
        statuses
    }

    pub fn process_pool_status(
        &mut self,
        status: Vec<anyhow::Result<MempoolStatus>>,
        producer_addr: &Addr<Producer>,
    ) -> Result<(u64, u64, MempoolAction), anyhow::Error> {
        let mut total_pending = 0;
        let mut total_queued = 0;
        for status in status.into_iter().flatten() {
            total_pending += status.pending;
            total_queued += status.queued;
        }

        let mut action = MempoolAction::None;

        // Check for mempool size limits
        if total_pending + total_queued > self.max_pool_size {
            producer_addr.do_send(PauseProducer);
            action = MempoolAction::Pause;
        } else if total_pending + total_queued < self.max_pool_size / 2 {
            producer_addr.do_send(ResumeProducer);
            action = MempoolAction::Resume;
        }

        // Check for high queued ratio (indicates nonce gaps)
        if total_pending > 0 {
            let ratio = total_queued as f64 / total_pending as f64;
            if ratio > self.queued_ratio_threshold {
                // Check cooldown
                if self.last_correction_check.elapsed() > self.correction_cooldown {
                    self.last_correction_check = std::time::Instant::now();
                    tracing::warn!(
                        "High queued/pending ratio detected: {:.2} (queued={}, pending={}), triggering nonce correction",
                        ratio,
                        total_queued,
                        total_pending
                    );
                    action = MempoolAction::NeedsNonceCorrection;
                }
            }
        }

        Ok((total_pending as u64, total_queued as u64, action))
    }

    /// Extract accounts with nonce gaps from txpool_content
    /// For each account in queued, the minimum nonce is what they're waiting for
    pub fn extract_nonce_corrections(content: &TxPoolContent) -> Vec<NonceCorrectionInfo> {
        let mut corrections = Vec::new();

        for (address, nonces) in &content.queued {
            // Find the minimum nonce in queued for this address
            if let Some(min_nonce) = nonces.keys().filter_map(|s| s.parse::<u64>().ok()).min() {
                // Check if this account has pending transactions
                let has_pending = content.pending.contains_key(address);
                
                if !has_pending && min_nonce > 0 {
                    // Account has queued txns but no pending, meaning nonce gap exists
                    // The expected nonce should be min_nonce (the one they're waiting for)
                    corrections.push(NonceCorrectionInfo {
                        account: *address,
                        expected_nonce: min_nonce,
                    });
                }
            }
        }

        tracing::info!(
            "Extracted {} nonce corrections from txpool_content",
            corrections.len()
        );

        corrections
    }
}

