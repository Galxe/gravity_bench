use std::{
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use actix::{Actor, Addr, AsyncContext, Context, Handler, ResponseFuture};
use alloy::primitives::keccak256;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tracing::{debug, error, info, warn};

use crate::{
    actors::{
        consumer::dispatcher::{Dispatcher, SimpleDispatcher},
        monitor::{RegisterConsumer, SubmissionResult, UpdateSubmissionResult},
        Monitor,
    },
    eth::EthHttpCli,
    txn_plan::SignedTxnWithMetadata,
};

// --- New: Retry configuration constants ---
/// Maximum retry attempts
const MAX_RETRIES: u32 = 3;
/// Retry delay
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Rate limiter configuration and state
#[derive(Clone)]
pub struct RateLimiter {
    /// Maximum transactions per second (0 means no limit)
    max_tps: u32,
    /// Token bucket capacity (usually equals max_tps)
    bucket_capacity: u32,
    /// Current tokens in bucket
    current_tokens: Arc<AtomicU64>,
    /// Last refill time
    last_refill: Arc<std::sync::Mutex<std::time::Instant>>,
}

impl RateLimiter {
    /// Create new rate limiter
    pub fn new(max_tps: u32) -> Self {
        let bucket_capacity = std::cmp::max(max_tps, 1); // At least 1 to avoid division by zero
        Self {
            max_tps,
            bucket_capacity,
            current_tokens: Arc::new(AtomicU64::new(bucket_capacity as u64)),
            last_refill: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        }
    }

    /// Create unlimited rate limiter (no rate limiting)
    pub fn unlimited() -> Self {
        Self {
            max_tps: 0,
            bucket_capacity: 0,
            current_tokens: Arc::new(AtomicU64::new(0)),
            last_refill: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        }
    }

    /// Try to acquire a token from the bucket
    /// Returns true if token acquired, false if rate limited
    pub fn try_acquire(&self) -> bool {
        // If no rate limit, always allow
        if self.max_tps == 0 {
            return true;
        }

        // Refill tokens based on elapsed time
        self.refill_tokens();

        // Try to consume a token
        let current = self.current_tokens.load(Ordering::Relaxed);
        if current > 0 {
            // Try to atomically decrement
            if self
                .current_tokens
                .compare_exchange_weak(current, current - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }

        false
    }

    /// Calculate wait time until next token is available
    pub fn wait_time_until_next_token(&self) -> Duration {
        if self.max_tps == 0 {
            return Duration::from_millis(0);
        }

        // If we have tokens, no wait needed
        if self.current_tokens.load(Ordering::Relaxed) > 0 {
            return Duration::from_millis(0);
        }

        // Calculate time per token: 1 second / max_tps
        let nanos_per_token = 1_000_000_000u64 / self.max_tps as u64;
        Duration::from_nanos(nanos_per_token)
    }

    /// Refill tokens based on elapsed time
    fn refill_tokens(&self) {
        let now = std::time::Instant::now();
        let mut last_refill = self.last_refill.lock().unwrap();
        let elapsed = now.duration_since(*last_refill);

        // Calculate tokens to add based on elapsed time
        let tokens_to_add = (elapsed.as_secs_f64() * self.max_tps as f64) as u64;

        if tokens_to_add > 0 {
            let current = self.current_tokens.load(Ordering::Relaxed);
            let new_tokens = std::cmp::min(current + tokens_to_add, self.bucket_capacity as u64);
            self.current_tokens.store(new_tokens, Ordering::Relaxed);
            *last_refill = now;
        }
    }

    /// Get current status
    pub fn get_status(&self) -> (u32, u32) {
        (
            self.current_tokens.load(Ordering::Relaxed) as u32,
            self.bucket_capacity,
        )
    }
}

/// Transaction Consumer Actor - Responsible for sending transactions to Ethereum nodes
pub struct Consumer {
    /// Provider dispatcher for sending transactions
    dispatcher: Arc<dyn Dispatcher>,
    /// Concurrency control semaphore
    semaphore: Arc<Semaphore>,
    /// Rate limiter for controlling TPS
    rate_limiter: RateLimiter,
    /// Statistics
    stats: ConsumerStats,
    /// Monitor address
    monitor_addr: Addr<Monitor>,
    /// Internal transaction pool sender
    pool_sender: mpsc::Sender<SignedTxnWithMetadata>,
    /// Transaction pool receiver
    pool_receiver: Option<mpsc::Receiver<SignedTxnWithMetadata>>,
    /// Pool maximum capacity
    max_pool_size: usize,
}

/// Consumer statistics
#[derive(Default, Clone)]
struct ConsumerStats {
    pub transactions_recv: u64,
    pub transactions_sent: Arc<AtomicU64>,
    pub transactions_sending: Arc<AtomicU64>,
    pub transactions_rejected: u64,
    pub transactions_rate_limited: Arc<AtomicU64>,
    pub pool_size: Arc<AtomicUsize>,
}

impl Consumer {
    /// Create new TxnConsumer instance
    pub fn new(
        dispatcher: Arc<dyn Dispatcher>,
        max_concurrent_senders: usize,
        monitor_addr: Addr<Monitor>,
        max_pool_size: usize,
        max_tps: Option<u32>,
    ) -> Self {
        let (pool_sender, pool_receiver) = mpsc::channel(max_pool_size);
        let rate_limiter = match max_tps {
            Some(tps) => RateLimiter::new(tps),
            None => RateLimiter::unlimited(),
        };

        Self {
            dispatcher,
            semaphore: Arc::new(Semaphore::new(max_concurrent_senders)),
            rate_limiter,
            stats: ConsumerStats::default(),
            monitor_addr,
            pool_sender,
            pool_receiver: Some(pool_receiver),
            max_pool_size,
        }
    }

    /// [Refactored Core] Handle single transaction sending with retry logic
    async fn process_transaction(
        _permit: OwnedSemaphorePermit,
        signed_txn: SignedTxnWithMetadata,
        dispatcher: Arc<dyn Dispatcher>,
        monitor_addr: Addr<Monitor>,
        transactions_sent: Arc<AtomicU64>,
        transactions_sending: Arc<AtomicU64>,
    ) {
        let metadata = signed_txn.metadata;
        debug!(
            "Acquired permit, processing transaction: {:?}",
            metadata.txn_id
        );
        transactions_sending.fetch_add(1, Ordering::Relaxed);

        let mut last_error: Option<anyhow::Error> = None;

        // --- New: Transaction sending retry loop ---
        for attempt in 1..=MAX_RETRIES {
            tracing::debug!(
                "Attempt {}/{} to send txn {:?}",
                attempt,
                MAX_RETRIES,
                metadata.txn_id
            );
            match dispatcher
                .send_tx(signed_txn.bytes.clone(), metadata.txn_id)
                .await
            {
                // Transaction sent successfully
                Ok((tx_hash, rpc_url)) => {
                    tracing::debug!(
                        "Txn sent successfully. Hash: from {} hash {}",
                        metadata.from_account,
                        tx_hash,
                    );
                    monitor_addr.do_send(UpdateSubmissionResult {
                        metadata,
                        result: Arc::new(SubmissionResult::Success(tx_hash)),
                        rpc_url,
                        send_time: Instant::now(),
                    });

                    // Update statistics and return early
                    transactions_sending.fetch_sub(1, Ordering::Relaxed);
                    transactions_sent.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                // Transaction sending failed, enter error handling and retry logic
                Err((e, url)) => {
                    let error_string = e.to_string().to_lowercase();

                    // --- Requirement 3: If it's an "underpriced" error ---
                    // This error means the transaction was accepted by the node but gas is insufficient. We can calculate the hash and treat it as successfully submitted.
                    if error_string.contains("underpriced") {
                        let tx_hash = keccak256(&signed_txn.bytes);
                        monitor_addr.do_send(UpdateSubmissionResult {
                            metadata,
                            result: Arc::new(SubmissionResult::Success(tx_hash)),
                            rpc_url: url,
                            send_time: Instant::now(),
                        });

                        transactions_sending.fetch_sub(1, Ordering::Relaxed);
                        transactions_sent.fetch_add(1, Ordering::Relaxed); // Count as sent
                        return;
                    }

                    // --- Requirement 2: If it's a Nonce related error ---
                    // "nonce too low" or "invalid nonce" suggests the on-chain nonce may have advanced
                    if error_string.contains("nonce too low")
                        || error_string.contains("invalid nonce")
                    {
                        // Check current on-chain nonce to determine final state
                        if let Ok(next_nonce) = dispatcher
                            .provider(&url)
                            .await
                            .unwrap()
                            .get_txn_count(metadata.from_account.as_ref().clone())
                            .await
                        {
                            // If on-chain nonce is greater than our attempted nonce, our transaction is indeed outdated
                            if next_nonce > metadata.nonce {
                                // Try to find the hash of the transaction using our nonce
                                let actual_nonce = metadata.nonce;
                                let from_account = metadata.from_account.clone();
                                // Can't find hash, but can provide correct nonce
                                monitor_addr.do_send(UpdateSubmissionResult {
                                    metadata,
                                    result: Arc::new(SubmissionResult::NonceTooLow {
                                        tx_hash: keccak256(&signed_txn.bytes),
                                        expect_nonce: next_nonce,
                                        actual_nonce,
                                        from_account,
                                    }),
                                    rpc_url: url,
                                    send_time: Instant::now(),
                                });
                            }
                        } else {
                            // Failed to get nonce, can only mark as retryable error
                            warn!("Failed to get nonce for txn {:?}: {}", metadata.txn_id, e);
                            monitor_addr.do_send(UpdateSubmissionResult {
                                metadata,
                                result: Arc::new(SubmissionResult::ErrorWithRetry),
                                rpc_url: "unknown".to_string(),
                                send_time: Instant::now(),
                            });
                        }
                        // After encountering Nonce error, should stop retrying and return regardless
                        transactions_sending.fetch_sub(1, Ordering::Relaxed);
                        return;
                    }

                    last_error = Some(e);

                    // If not the last attempt, wait then retry
                    if attempt < MAX_RETRIES {
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
        }

        // --- Final handling after all retries failed ---
        error!(
            "Txn {:?} failed after {} retries. Last error: {:?}, account: {:?}, nonce: {:?}",
            metadata.txn_id,
            MAX_RETRIES,
            last_error.map(|e| e.to_string()),
            metadata.from_account,
            metadata.nonce
        );

        monitor_addr.do_send(UpdateSubmissionResult {
            metadata,
            result: Arc::new(SubmissionResult::ErrorWithRetry), // Mark as needing upstream retry
            rpc_url: "unknown".to_string(),
            send_time: Instant::now(),
        });

        transactions_sending.fetch_sub(1, Ordering::Relaxed);
    }

    /// Create new TxnConsumer instance (convenience method, using SimpleDispatcher)
    pub fn new_with_providers(
        providers: Vec<EthHttpCli>,
        max_concurrent_senders: usize,
        monitor_addr: Addr<Monitor>,
        max_pool_size: usize,
        max_tps: Option<u32>,
    ) -> Consumer {
        let dispatcher = Arc::new(SimpleDispatcher::new(providers));
        Consumer::new(
            dispatcher,
            max_concurrent_senders,
            monitor_addr,
            max_pool_size,
            max_tps,
        )
    }

    /// Start transaction pool consumer
    fn start_pool_consumer(
        &self,
        mut pool_receiver: mpsc::Receiver<SignedTxnWithMetadata>,
        transactions_sent: Arc<AtomicU64>, // Number of successfully sent transactions
        pool_size: Arc<AtomicUsize>,       // Transaction pool size
    ) {
        let dispatcher = self.dispatcher.clone();
        let semaphore = self.semaphore.clone();
        let monitor_addr = self.monitor_addr.clone();
        let transactions_sending = self.stats.transactions_sending.clone();
        let rate_limiter = self.rate_limiter.clone();
        let rate_limited_count = self.stats.transactions_rate_limited.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                info!("Transaction pool consumer started with JoinSet and rate limiting (max_tps: {}).", 
                      if rate_limiter.max_tps == 0 { "unlimited".to_string() } else { rate_limiter.max_tps.to_string() });
                let mut in_flight_tasks = tokio::task::JoinSet::new();

                loop {
                    tokio::select! {
                        biased; // Prefer processing completed tasks to release semaphore permits

                        // Branch 1: A flying task completed
                        Some(result) = in_flight_tasks.join_next(), if !in_flight_tasks.is_empty() => {
                            if let Err(e) = result {
                                error!("A transaction processing task panicked or was cancelled: {:?}", e);
                            }
                        }

                        // Branch 2: Received a new transaction from the pool
                        Some(signed_txn) = pool_receiver.recv() => {
                            pool_size.fetch_sub(1, Ordering::Relaxed);
                            debug!("Processing txn {:?}, pool size is now {}", signed_txn.metadata.txn_id, pool_size.load(Ordering::Relaxed));

                            // --- NEW: Rate limiting logic ---
                            if !rate_limiter.try_acquire() {
                                // Rate limited - wait for next available slot
                                let wait_time = rate_limiter.wait_time_until_next_token();
                                if wait_time > Duration::from_millis(0) {
                                    debug!("Rate limited, waiting {:?} before processing txn {:?}", wait_time, signed_txn.metadata.txn_id);
                                    rate_limited_count.fetch_add(1, Ordering::Relaxed);
                                    tokio::time::sleep(wait_time).await;
                                    // Try again after waiting
                                    if !rate_limiter.try_acquire() {
                                        // Still rate limited after waiting, this shouldn't happen often
                                        warn!("Still rate limited after waiting for txn {:?}", signed_txn.metadata.txn_id);
                                        tokio::time::sleep(Duration::from_millis(100)).await; // Small additional delay
                                    }
                                }
                            }

                            let permit = match semaphore.clone().acquire_owned().await {
                                Ok(permit) => permit,
                                Err(_) => {
                                    error!("Semaphore has been closed, stopping consumer loop.");
                                    monitor_addr.do_send(UpdateSubmissionResult {
                                        metadata: signed_txn.metadata,
                                        result: Arc::new(SubmissionResult::ErrorWithRetry),
                                        rpc_url: "unknown".to_string(),
                                        send_time: Instant::now(),
                                        });
                                    break;
                                }
                            };

                            // [No changes] Put processing task into JoinSet
                            in_flight_tasks.spawn(Self::process_transaction(
                                permit,
                                signed_txn,
                                dispatcher.clone(),
                                monitor_addr.clone(),
                                transactions_sent.clone(),
                                transactions_sending.clone(),
                            ));
                        }

                        // Branch 3: Transaction pool is closed and all flying tasks are completed
                        else => {
                            info!("Transaction pool closed and all in-flight tasks finished.");
                            break;
                        }
                    }
                }
            });
        });
    }
}

// --- Actor, Handler and other implementations remain unchanged ---

impl Actor for Consumer {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Register self with Monitor
        self.monitor_addr.do_send(RegisterConsumer {
            addr: ctx.address(),
        });

        let rate_limiter = self.rate_limiter.clone();
        let dispatcher = self.dispatcher.clone();
        ctx.run_interval(Duration::from_secs(5), move |act, _ctx| {
            let (current_tokens, bucket_capacity) = rate_limiter.get_status();
            debug!(
                "Consumer Stats: recv: {}, sending: {}, sent: {}, rejected: {}, rate_limited: {}, pool_size: {}, rate_limiter: {}/{}",
                act.stats.transactions_recv,
                act.stats.transactions_sending.load(Ordering::Relaxed),
                act.stats.transactions_sent.load(Ordering::Relaxed),
                act.stats.transactions_rejected,
                act.stats.transactions_rate_limited.load(Ordering::Relaxed),
                act.stats.pool_size.load(Ordering::Relaxed),
                current_tokens,
                bucket_capacity,
            );
            let providers = dispatcher.get_providers();
            actix::spawn(async move {
                for p in providers {
                    p.log_metrics_summary().await;
                }
            });
        });

        let tps_info = if self.rate_limiter.max_tps == 0 {
            "unlimited TPS".to_string()
        } else {
            format!("{} TPS", self.rate_limiter.max_tps)
        };

        info!(
            "Consumer actor started with {} max concurrent senders, pool size {}, and {}",
            self.semaphore.available_permits(),
            self.max_pool_size,
            tps_info
        );

        let pool_receiver = self.pool_receiver.take().unwrap();

        self.start_pool_consumer(
            pool_receiver,
            self.stats.transactions_sent.clone(),
            self.stats.pool_size.clone(),
        );
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("Consumer actor stopped");
    }
}

impl Handler<SignedTxnWithMetadata> for Consumer {
    type Result = ResponseFuture<anyhow::Result<()>>;

    fn handle(&mut self, msg: SignedTxnWithMetadata, _ctx: &mut Self::Context) -> Self::Result {
        debug!("Received signed transaction: {:?}", msg.metadata.txn_id);
        self.stats.transactions_recv += 1;
        let sender = self.pool_sender.clone();
        let pool_size = self.stats.pool_size.clone();
        Box::pin(async move {
            match sender.send(msg).await {
                Ok(_) => {
                    pool_size.fetch_add(1, Ordering::Relaxed);
                    debug!(
                        "Transaction added to pool, current pool size: {}",
                        pool_size.load(Ordering::Relaxed)
                    );
                    Ok(())
                }
                Err(e) => Err(anyhow::anyhow!("Pool send error: {:?}", e)),
            }
        })
    }
}
