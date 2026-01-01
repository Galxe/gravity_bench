use std::{collections::HashMap, sync::Arc};

use actix::Addr;

use crate::{
    actors::{PauseProducer, Producer, ResumeProducer},
    eth::{EthHttpCli, MempoolStatus, TxPoolContent},
};



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

        // Check for high queued transactions (indicates nonce gaps)
        if total_queued > 0 {
            // Calculate ratio, handling division by zero if pending is 0
            let ratio = if total_pending > 0 {
                total_queued as f64 / total_pending as f64
            } else {
                f64::INFINITY
            };

            // Trigger if ratio is high OR if we have significant queued transactions with low pending
            if ratio > self.queued_ratio_threshold || (total_queued > 100 && total_pending < 10) {
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

    /// Identify accounts with nonce gaps from txpool_content
    /// Returns list of addresses that need correction
    pub fn identify_problematic_accounts(content: &TxPoolContent) -> Vec<alloy::primitives::Address> {
        let mut problematic_accounts = Vec::new();

        for (address, nonces) in &content.queued {
            // Identify accounts that have queued transactions.
            // Presence in 'queued' implies a gap or issue.
            // We check if they also have pending transactions.
            // If they have NO pending transactions but HAVE queued, strictly implies a gap.
            let has_pending = content.pending.contains_key(address);
            
            // Check if there are any valid nonces in queued
             if nonces.keys().any(|s| s.parse::<u64>().is_ok()) {
                if !has_pending {
                     problematic_accounts.push(*address);
                }
            }
        }

        tracing::info!(
            "Identified {} accounts with likely nonce gaps",
            problematic_accounts.len()
        );

        problematic_accounts
    }
}

