use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::TxHash;
use tracing::{debug, error, warn};
use comfy_table::{Table, Cell, presets::UTF8_FULL, Attribute, Color};

use crate::actors::monitor::SubmissionResult;
use crate::eth::EthHttpCli;
use crate::txn_plan::{PlanId, TxnMetadata};

use super::UpdateSubmissionResult;

const SAMPLING_SIZE: usize = 10; // Define sampling size
const TXN_TIMEOUT: Duration = Duration::from_secs(600); // 10 minutes timeout
const TPS_WINDOW: Duration = Duration::from_secs(17);

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

/// Transaction and plan lifecycle tracker
pub struct TxnTracker {
    /// Plan tracker
    plan_trackers: HashMap<PlanId, PlanTracker>,
    /// Time-sorted set of in-flight transactions
    /// Using BTreeSet allows automatic sorting based on our implemented Ord
    pending_txns: BTreeSet<PendingTxInfo>,
    /// RPC client mapping
    clients: HashMap<String, Arc<EthHttpCli>>,
    /// Timestamps of resolved transactions for TPS calculation
    resolved_txn_timestamps: VecDeque<Instant>,
    total_produced_transactions: u64,
    total_resolved_transactions: u64,
    total_failed_submissions: u64,
    total_failed_executions: u64,
    last_completed_plan: Option<(PlanId, PlanTracker)>,
}

/// Tracking status of a single transaction plan
#[derive(Debug, Clone)]
struct PlanTracker {
    /// Total number of transactions in the plan
    produce_transactions: usize,
    /// Number of transactions that have reached final state
    resolved_transactions: u64,
    /// Number of consumed transactions
    consumed_transactions: u64,
    /// Number of failed submissions
    failed_submissions: u64,
    /// Number of failed executions (reverted)
    failed_executions: u64,

    plan_produced: bool,

    plan_name: String,
}

/// Detailed information of in-flight transactions
/// Added Eq, Ord and other Trait implementations to enable storage and sorting in BTreeSet
#[derive(Debug, Clone)]
pub(crate) struct PendingTxInfo {
    tx_hash: TxHash,
    metadata: Arc<TxnMetadata>,
    rpc_url: String,
    submit_time: Instant,
}

//--- Core implementation required for BTreeSet sorting ---//

impl PartialEq for PendingTxInfo {
    fn eq(&self, other: &Self) -> bool {
        self.submit_time == other.submit_time && self.tx_hash == other.tx_hash
    }
}
impl Eq for PendingTxInfo {}

impl PartialOrd for PendingTxInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingTxInfo {
    /// Sorting rules:
    /// 1. Primarily sorted by submission time (`submit_time`) in ascending order
    /// 2. If submission times are the same, sort by transaction hash (`tx_hash`) to ensure uniqueness
    fn cmp(&self, other: &Self) -> Ordering {
        self.submit_time
            .cmp(&other.submit_time)
            .then_with(|| self.tx_hash.cmp(&other.tx_hash))
    }
}

/// Plan completion status
#[derive(Debug)]
pub enum PlanStatus {
    Completed,
    Failed { reason: String },
    InProgress,
}

impl TxnTracker {
    /// Create new transaction tracker
    pub fn new(clients: Vec<Arc<EthHttpCli>>) -> Self {
        let mut client_map = HashMap::new();
        for client in clients {
            let rpc_url = client.rpc().as_ref().clone();
            client_map.insert(rpc_url, client);
        }

        Self {
            plan_trackers: HashMap::new(),
            pending_txns: BTreeSet::new(),
            clients: client_map,
            resolved_txn_timestamps: VecDeque::new(),
            total_produced_transactions: 0,
            total_resolved_transactions: 0,
            total_failed_submissions: 0,
            total_failed_executions: 0,
            last_completed_plan: None,
        }
    }

    pub fn handler_produce_txns(&mut self, plan_id: PlanId, count: usize) {
        if let Some(tracker) = self.plan_trackers.get_mut(&plan_id) {
            tracker.produce_transactions += count;
            self.total_produced_transactions += count as u64;
        }
    }

    pub fn handle_plan_produced(&mut self, plan_id: PlanId) {
        if let Some(tracker) = self.plan_trackers.get_mut(&plan_id) {
            tracker.plan_produced = true;
        }
    }

    /// Register new plan (no changes)
    pub fn register_plan(&mut self, plan_id: PlanId, plan_name: String) {
        debug!("Plan registered: plan_id={}", plan_id);
        let tracker = PlanTracker {
            produce_transactions: 0,
            resolved_transactions: 0,
            consumed_transactions: 0,
            failed_submissions: 0,
            failed_executions: 0,
            plan_produced: false,
            plan_name,
        };
        self.plan_trackers.insert(plan_id, tracker);
    }

    /// Handle transaction submission result
    pub fn handle_submission_result(&mut self, msg: &UpdateSubmissionResult) {
        let plan_id = &msg.metadata.plan_id;
        if !self.plan_trackers.contains_key(plan_id) {
            warn!("Plan not found: plan_id={}", plan_id);
            return;
        }
        let plan_tracker = self.plan_trackers.get_mut(plan_id).unwrap();

        plan_tracker.consumed_transactions += 1;
        match msg.result.as_ref() {
            SubmissionResult::Success(tx_hash) => {
                debug!(
                    "Transaction submitted successfully: plan_id={}, tx_hash={:?}, rpc_url={}",
                    plan_id, tx_hash, msg.rpc_url
                );

                let pending_info = PendingTxInfo {
                    tx_hash: *tx_hash,
                    metadata: msg.metadata.clone(),
                    rpc_url: msg.rpc_url.clone(),
                    submit_time: Instant::now(),
                };

                // Insert transaction into the global, time-sorted BTreeSet
                self.pending_txns.insert(pending_info);
            }
            SubmissionResult::NonceTooLow((nonce, tx_hash)) => {
                let pending_info = PendingTxInfo {
                    tx_hash: *tx_hash,
                    metadata: msg.metadata.clone(),
                    rpc_url: msg.rpc_url.clone(),
                    submit_time: Instant::now(),
                };
                self.pending_txns.insert(pending_info);
                warn!(
                    "Transaction submission failed because nonce is too low: plan_id={}, nonce={}, tx_hash={:?}",
                    plan_id, nonce, tx_hash
                );
            }
            e => {
                warn!(
                    "Transaction submission failed: plan_id={}, error={:?}",
                    plan_id, e
                );
                if let Some(tracker) = self.plan_trackers.get_mut(plan_id) {
                    tracker.resolved_transactions += 1;
                    tracker.failed_submissions += 1;
                    self.total_failed_submissions += 1;
                    self.resolved_txn_timestamps.push_back(Instant::now());
                    self.total_resolved_transactions += 1;
                }
            }
        }
    }

    /// Check if plan is completed (no changes)
    pub fn check_plan_completion(&mut self, plan_id: &PlanId) -> PlanStatus {
        if let Some(tracker) = self.plan_trackers.get(plan_id) {
            debug!("Plan {}({}) status: produce_transactions={}, consumed_transactions={}, resolved_transactions={}, failed_submissions={}, failed_executions={}", 
                tracker.plan_name, plan_id, tracker.produce_transactions, tracker.consumed_transactions, tracker.resolved_transactions, tracker.failed_submissions, tracker.failed_executions);
            if tracker.produce_transactions != 0
                && tracker.resolved_transactions as usize >= tracker.produce_transactions
                && tracker.plan_produced
            {
                let has_failures = tracker.failed_submissions > 0 || tracker.failed_executions > 0;
                let status = if has_failures {
                    let reason = format!(
                        "Plan failed: {} submission failures, {} execution failures",
                        tracker.failed_submissions, tracker.failed_executions
                    );
                    warn!("Plan {} failed: {}", tracker.plan_name, reason);
                    PlanStatus::Failed { reason }
                } else {
                    debug!("Plan {} completed successfully", plan_id);
                    PlanStatus::Completed
                };
                if let Some(completed_tracker) = self.plan_trackers.remove(plan_id) {
                    self.last_completed_plan = Some((plan_id.clone(), completed_tracker));
                }
                status
            } else {
                PlanStatus::InProgress
            }
        } else {
            PlanStatus::InProgress
        }
    }

    /// Get all active plan IDs being tracked (no changes)
    pub fn get_active_plan_ids(&self) -> Vec<PlanId> {
        self.plan_trackers.keys().cloned().collect()
    }

    pub fn perform_sampling_check(
        &mut self,
    ) -> Vec<
        impl std::future::Future<
            Output = (
                PendingTxInfo,
                Result<Option<alloy::rpc::types::TransactionReceipt>, anyhow::Error>,
            ),
        >,
    > {
        let total_pending = self.pending_txns.len();
        if total_pending == 0 {
            return Vec::new();
        }

        let mut samples = BTreeSet::new(); // Use BTreeSet to avoid duplicates if indices overlap
        let mut tasks = Vec::new();

        // --- Core sampling logic ---
        if total_pending <= SAMPLING_SIZE {
            // If total is less than sampling size, check all
            samples.extend(self.pending_txns.iter().cloned());
        } else {
            // Select samples at fixed intervals in the queue
            // For example, with 1000 txns and 10 samples, take one every 100
            let step = total_pending / SAMPLING_SIZE;
            for i in 0..SAMPLING_SIZE {
                let index = i * step;
                if let Some(txn_info) = self.pending_txns.iter().nth(index) {
                    samples.insert(txn_info.clone());
                }
            }
            // Always include the oldest one as it's most critical
            if let Some(oldest) = self.pending_txns.iter().next() {
                samples.insert(oldest.clone());
            }
        }

        for pending_info in samples {
            if let Some(client) = self.clients.get(&pending_info.rpc_url) {
                let client = client.clone();
                let task_info = pending_info.clone();

                let task = async move {
                    let result = client.get_transaction_receipt(task_info.tx_hash).await;
                    debug!(
                        "checked tx_hash={:?} result={:?}",
                        task_info.tx_hash, result
                    );
                    (task_info, result)
                };
                tasks.push(task);
            } else {
                warn!("No client found for RPC URL: {}", pending_info.rpc_url);
            }
        }

        tasks
    }

    pub fn handle_receipt_result(
        &mut self,
        results: Vec<(
            PendingTxInfo,
            Result<Option<alloy::rpc::types::TransactionReceipt>, anyhow::Error>,
        )>,
    ) {
        let mut successful_txns = Vec::new();
        let mut failed_txns = Vec::new(); // Including Pending, Timeout, Error

        // 1. Categorize results
        for (info, result) in results {
            match result {
                Ok(Some(receipt)) => {
                    // Transaction successfully confirmed
                    self.pending_txns.remove(&info);
                    successful_txns.push((info, receipt));
                }
                Ok(None) => {
                    // Transaction still pending
                    failed_txns.push(info);
                }
                Err(e) => {
                    // RPC query failed
                    warn!(
                        "Failed to get receipt for tx_hash={:?}: {}",
                        info.tx_hash, e
                    );
                    failed_txns.push(info);
                }
            }
        }

        if !failed_txns.is_empty() {
            debug!(
                "Failed to get receipt for {} transactions",
                failed_txns.len()
            );
        }

        let successful_txns_hash = successful_txns
            .iter()
            .map(|(info, _)| info.tx_hash)
            .collect::<HashSet<_>>();

        // 2. If there are successful transactions, calculate median time and clean up
        if !successful_txns.is_empty() {
            // Create a temporary TxnInfo for BTreeSet split_off
            // TxHash is not important as sorting is mainly based on time
            let split_info = successful_txns[successful_txns.len() - 1].0.clone();

            // Use split_off to efficiently split BTreeSet
            // `cleared_txns` contains all transactions with time <= median_time
            let cleared_txns = self.pending_txns.split_off(&split_info);

            // Process transactions that were batch cleaned
            for cleared_info in self.pending_txns.iter() {
                if successful_txns_hash.contains(&cleared_info.tx_hash) {
                    continue;
                }
                if let Some(plan_tracker) =
                    self.plan_trackers.get_mut(&cleared_info.metadata.plan_id)
                {
                    plan_tracker.resolved_transactions += 1;
                    self.resolved_txn_timestamps.push_back(Instant::now());
                    self.total_resolved_transactions += 1;
                }
            }

            // Update main queue with the remaining part (newer transactions)
            self.pending_txns = cleared_txns;
        }

        // 3. Process the confirmed transactions from this sampling
        for (info, receipt) in successful_txns {
            if let Some(plan_tracker) = self.plan_trackers.get_mut(&info.metadata.plan_id) {
                plan_tracker.resolved_transactions += 1;
                self.resolved_txn_timestamps.push_back(Instant::now());
                self.total_resolved_transactions += 1;
                if !receipt.status() {
                    plan_tracker.failed_executions += 1;
                    self.total_failed_executions += 1;
                    warn!(
                        "Transaction reverted: plan_id={}, tx_hash={:?}",
                        info.metadata.plan_id, info.tx_hash
                    );
                }
            }
        }

        // 4. Process failed or still pending transactions from this sampling
        for info in failed_txns {
            if info.submit_time.elapsed() > TXN_TIMEOUT {
                // Transaction timed out, completely failed
                error!(
                    "Transaction timed out: plan_id={}, tx_hash={:?}",
                    info.metadata.plan_id, info.tx_hash
                );
                if let Some(plan_tracker) = self.plan_trackers.get_mut(&info.metadata.plan_id) {
                    plan_tracker.resolved_transactions += 1;
                    plan_tracker.failed_executions += 1;
                    self.total_failed_executions += 1;
                    self.resolved_txn_timestamps.push_back(Instant::now());
                    self.total_resolved_transactions += 1;
                }
            } else {
                // Not timed out, put back in main queue for next round check
                debug!(
                    "Re-inserting pending transaction: tx_hash={:?}",
                    info.tx_hash
                );
                self.pending_txns.insert(info);
            }
        }
    }

    pub fn log_stats(&mut self) {
        // Update TPS window by removing timestamps older than 30 seconds
        let now = Instant::now();
        let window_start = now - TPS_WINDOW;
        while let Some(ts) = self.resolved_txn_timestamps.front() {
            if *ts < window_start {
                self.resolved_txn_timestamps.pop_front();
            } else {
                break;
            }
        }

        // Calculate TPS
        let tps = self.resolved_txn_timestamps.len() as f64 / TPS_WINDOW.as_secs_f64();

        let mut plan_summaries = Vec::new();

        if !self.plan_trackers.is_empty() {
            for (_plan_id, tracker) in &self.plan_trackers {
                plan_summaries.push(format!(
                    "{}: {}/{}",
                    tracker.plan_name,
                    tracker.resolved_transactions,
                    tracker.produce_transactions
                ));
            }
        } else if let Some((_plan_id, tracker)) = &self.last_completed_plan {
            plan_summaries.push(format!(
                "{}: {}/{} done",
                tracker.plan_name,
                tracker.resolved_transactions,
                tracker.produce_transactions
            ));
        }

        // Calculate success rate
        let success_rate = if self.total_produced_transactions > 0 {
            self.total_resolved_transactions as f64 / self.total_produced_transactions as f64 * 100.0
        } else {
            0.0
        };

        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        
        // Set table header
        table.set_header(vec![
            "Plan Name",
            "Progress", 
            "Success%",
            "SubFail",
            "ExecFail",
            "Status"
        ]);
        
        // Add individual plan rows
        if !self.plan_trackers.is_empty() {
            for (_plan_id, tracker) in &self.plan_trackers {
                let plan_success_rate = if tracker.resolved_transactions > 0 {
                    let successful = tracker.resolved_transactions.saturating_sub(tracker.failed_submissions + tracker.failed_executions);
                    successful as f64 / tracker.resolved_transactions as f64 * 100.0
                } else if tracker.produce_transactions > 0 {
                    // If no transactions resolved yet, can't calculate success rate
                    0.0
                } else {
                    100.0
                };
                
                let progress_color = if tracker.resolved_transactions as usize >= tracker.produce_transactions {
                    Color::Green
                } else {
                    Color::Yellow
                };
                
                let status = if tracker.plan_produced && tracker.resolved_transactions as usize >= tracker.produce_transactions {
                    if tracker.failed_submissions + tracker.failed_executions > 0 {
                        "Completed (w/ Errors)"
                    } else {
                        "Completed"
                    }
                } else {
                    "In Progress"
                };
                
                table.add_row(vec![
                    Cell::new(&tracker.plan_name).fg(Color::Cyan),
                    Cell::new(&format!("{}/{}", 
                        format_large_number(tracker.resolved_transactions),
                        format_large_number(tracker.produce_transactions as u64)
                    )).fg(progress_color),
                    Cell::new(&format!("{:.1}", plan_success_rate)).fg(if plan_success_rate >= 95.0 { Color::Green } else if plan_success_rate >= 80.0 { Color::Yellow } else { Color::Red }),
                    Cell::new(&format_large_number(tracker.failed_submissions)).fg(if tracker.failed_submissions > 0 { Color::Red } else { Color::Green }),
                    Cell::new(&format_large_number(tracker.failed_executions)).fg(if tracker.failed_executions > 0 { Color::Red } else { Color::Green }),
                    Cell::new(status).fg(if status.contains("Completed") { Color::Green } else { Color::Yellow }),
                ]);
            }
        } else if let Some((_plan_id, tracker)) = &self.last_completed_plan {
            let plan_success_rate = if tracker.resolved_transactions > 0 {
                let successful = tracker.resolved_transactions.saturating_sub(tracker.failed_submissions + tracker.failed_executions);
                successful as f64 / tracker.resolved_transactions as f64 * 100.0
            } else {
                100.0
            };
            
            table.add_row(vec![
                Cell::new(&format!("{} (Last)", tracker.plan_name)).fg(Color::DarkGrey),
                Cell::new(&format!("{}/{}", 
                    format_large_number(tracker.resolved_transactions),
                    format_large_number(tracker.produce_transactions as u64)
                )).fg(Color::Green),
                Cell::new(&format!("{:.1}", plan_success_rate)).fg(Color::Green),
                Cell::new(&format_large_number(tracker.failed_submissions)).fg(if tracker.failed_submissions > 0 { Color::Red } else { Color::Green }),
                Cell::new(&format_large_number(tracker.failed_executions)).fg(if tracker.failed_executions > 0 { Color::Red } else { Color::Green }),
                Cell::new("Done").fg(Color::Green),
            ]);
        }
        
        // Add summary row
        table.add_row(vec![
            Cell::new("TOTAL").add_attribute(Attribute::Bold).fg(Color::Blue),
            Cell::new(&format!("{}/{}", 
                format_large_number(self.total_resolved_transactions),
                format_large_number(self.total_produced_transactions)
            )).add_attribute(Attribute::Bold).fg(Color::Blue),
            Cell::new(&format!("{:.1}", success_rate)).add_attribute(Attribute::Bold).fg(Color::Blue),
            Cell::new(&format_large_number(self.total_failed_submissions)).add_attribute(Attribute::Bold).fg(Color::Blue),
            Cell::new(&format_large_number(self.total_failed_executions)).add_attribute(Attribute::Bold).fg(Color::Blue),
            Cell::new(&format!("TPS:{:.1}", tps)).add_attribute(Attribute::Bold).fg(Color::Magenta),
        ]);
        
        println!("{}", table);
    }
}
