use actix::prelude::*;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::actors::consumer::Consumer;
use crate::actors::monitor::monitor_actor::{PlanProduced, ProduceTxns};
use crate::actors::monitor::{
    Monitor, PlanCompleted, PlanFailed, RegisterPlan, RegisterProducer, ReportProducerStats,
    SubmissionResult, UpdateSubmissionResult,
};
use crate::actors::{ExeFrontPlan, PauseProducer, ResumeProducer};
use crate::txn_plan::{addr_pool::AddressPool, PlanExecutionMode, PlanId, TxnPlan};
use crate::util::gen_account::{AccountId, AccountManager};

use super::messages::RegisterTxnPlan;

pub struct ProducerStats {
    pub remain_plans_num: u64,
    pub success_plans_num: u64,
    pub failed_plans_num: u64,
    pub sending_txns: Arc<AtomicU64>,
    pub success_txns: u64,
    pub failed_txns: u64,
    pub ready_accounts: Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
pub struct ProducerState {
    state: Arc<AtomicU32>,
}

impl ProducerState {
    pub fn running() -> Self {
        Self {
            state: Arc::new(AtomicU32::new(1)),
        }
    }

    pub fn set_running(&self) {
        self.state.store(1, Ordering::Relaxed);
    }

    pub fn set_paused(&self) {
        self.state.store(0, Ordering::Relaxed);
    }

    pub fn is_running(&self) -> bool {
        self.state.load(Ordering::Relaxed) == 1
    }

    pub fn is_paused(&self) -> bool {
        self.state.load(Ordering::Relaxed) == 0
    }
}

/// The main TxnProducer actor, responsible for executing transaction plans sequentially.
pub struct Producer {
    /// Manages account nonces and readiness.
    address_pool: Arc<dyn AddressPool>,
    /// The current state of the producer (Running or Paused).
    state: ProducerState,

    stats: ProducerStats,

    /// Actor addresses for communication with other parts of the system.
    monitor_addr: Addr<Monitor>,
    consumer_addr: Addr<Consumer>,

    nonce_cache: Arc<DashMap<AccountId, u32>>,

    account_generator: AccountManager,

    /// A queue of plans waiting to be executed. Plans are processed in FIFO order.
    /// Only this queue is subject to max_queue_size limit.
    plan_queue: VecDeque<Box<dyn TxnPlan>>,
    /// Plans that have finished producing transactions but are awaiting completion confirmation.
    /// These do NOT count towards the queue size limit.
    awaiting_completion: HashSet<PlanId>,
    /// Global responder management across all plan states.
    plan_responders: HashMap<PlanId, tokio::sync::oneshot::Sender<Result<(), anyhow::Error>>>,
    /// The maximum number of plans allowed in the queue (only applies to plan_queue).
    max_queue_size: usize,
}

impl Producer {
    pub async fn new(
        address_pool: Arc<dyn AddressPool>,
        consumer_addr: Addr<Consumer>,
        monitor_addr: Addr<Monitor>,
        account_generator: AccountManager,
    ) -> Result<Self, anyhow::Error> {
        let nonce_cache = Arc::new(DashMap::new());
        address_pool.clean_ready_accounts();
        for (account_id, nonce) in account_generator.account_ids_with_nonce() {
            let nonce = nonce.load(Ordering::Relaxed) as u32;
            nonce_cache.insert(account_id, nonce);
            address_pool.unlock_correct_nonce(account_id, nonce);
        }
        Ok(Self {
            state: ProducerState::running(),
            stats: ProducerStats {
                remain_plans_num: 0,
                sending_txns: Arc::new(AtomicU64::new(0)),
                ready_accounts: Arc::new(AtomicU64::new(0)),
                success_plans_num: 0,
                failed_plans_num: 0,
                success_txns: 0,
                failed_txns: 0,
            },
            address_pool,
            nonce_cache,
            account_generator,
            monitor_addr,
            consumer_addr,
            plan_queue: VecDeque::new(),
            awaiting_completion: HashSet::new(),
            plan_responders: HashMap::new(),
            max_queue_size: 10,
        })
    }

    #[allow(unused)]
    pub fn with_max_queue_size(mut self, max_queue_size: usize) -> Self {
        self.max_queue_size = max_queue_size;
        self
    }

    /// Attempts to trigger the execution of the plan at the front of the queue.
    /// This method should be called whenever there's a state change that might
    /// allow a waiting plan to proceed (e.g., a plan completes, an account becomes ready).
    fn trigger_next_plan_if_needed(&self, ctx: &mut Context<Self>) {
        if self.state.is_running() && !self.plan_queue.is_empty() {
            ctx.address().do_send(ExeFrontPlan);
        }
    }

    /// Checks if the required accounts for a given plan are ready.
    async fn check_plan_ready(
        plan_mode: &PlanExecutionMode,
        address_pool: &Arc<dyn AddressPool>,
    ) -> bool {
        match plan_mode {
            PlanExecutionMode::Full => address_pool.is_full_ready(),
            PlanExecutionMode::Partial(required_count) => {
                address_pool.ready_len() >= *required_count
            }
        }
    }

    /// The core logic for executing a single transaction plan.
    /// This is a static method to make dependencies explicit.
    async fn execute_plan(
        monitor_addr: Addr<Monitor>,
        consumer_addr: Addr<Consumer>,
        address_pool: Arc<dyn AddressPool>,
        account_generator: AccountManager,
        mut plan: Box<dyn TxnPlan>,
        sending_txns: Arc<AtomicU64>,
        state: ProducerState,
        nonce_cache: Arc<DashMap<AccountId, u32>>,
    ) -> Result<(), anyhow::Error> {
        let plan_id = plan.id().clone();

        // Fetch accounts and build transactions
        let ready_accounts =
            address_pool.fetch_senders(plan.size().unwrap_or_else(|| address_pool.len()));
        let iterator = plan
            .as_mut()
            .build_txns(ready_accounts, account_generator.clone())?;

        // If the plan doesn't consume nonces, accounts can be used by other processes immediately.
        if !iterator.consume_nonce {
            address_pool.resume_all_accounts();
        }
        // must send to monitor before sending to consumer
        monitor_addr
            .send(RegisterPlan {
                plan_id: plan_id.clone(),
                plan_name: plan.name().to_string(),
            })
            .await
            .unwrap();
        let mut count = 0;
        // Send all signed transactions to the consumer
        while let Ok(signed_txn) = iterator.iterator.recv() {
            while state.is_paused() {
                tracing::debug!("Producer is paused");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let account_id = signed_txn.metadata.from_account_id;
            let next_nonce = match nonce_cache.get(&account_id) {
                Some(nonce) => *nonce,
                None => 0,
            };
            if let Err(e) = consumer_addr.send(signed_txn).await {
                // If sending to the consumer fails, we abort the entire plan.
                tracing::error!(
                    "Consumer actor send error, aborting plan '{}': {}",
                    plan.name(),
                    e
                );
                return Err(anyhow::anyhow!(
                    "Failed to send transaction to Consumer: {}",
                    e
                ));
            }
            monitor_addr.do_send(ProduceTxns {
                plan_id: plan_id.clone(),
                count: 1,
            });
            count += 1;
            sending_txns.fetch_add(1, Ordering::Relaxed);
        }
        monitor_addr.do_send(PlanProduced {
            plan_id: plan_id.clone(),
            count,
        });

        tracing::debug!(
            "All transactions for plan '{}' (id={}) ({} txns) have been sent to the consumer.",
            plan.name(),
            plan_id,
            count,
        );

        Ok(())
    }
}

impl Actor for Producer {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.monitor_addr.do_send(RegisterProducer {
            addr: ctx.address(),
        });
        let address_pool = self.address_pool.clone();
        async move {
            let count = address_pool.len();
            tracing::info!("Producer started with {} accounts.", count);
        }
        .into_actor(self)
        .wait(ctx);
        ctx.run_interval(Duration::from_secs(1), |act, _ctx| {
            act.monitor_addr.do_send(ReportProducerStats {
                ready_accounts: act.stats.ready_accounts.load(Ordering::Relaxed),
                sending_txns: act.stats.sending_txns.load(Ordering::Relaxed),
            });
            tracing::debug!("Producer stats: plans_num={}, sending_txns={}, ready_accounts={}, success_plans_num={}, failed_plans_num={}, success_txns={}, failed_txns={}", act.stats.remain_plans_num, act.stats.sending_txns.load(Ordering::Relaxed), act.stats.ready_accounts.load(Ordering::Relaxed), act.stats.success_plans_num, act.stats.failed_plans_num, act.stats.success_txns, act.stats.failed_txns);
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("Producer stopped.");
    }
}

/// Handler for the message that triggers the execution of the front-most plan.
impl Handler<ExeFrontPlan> for Producer {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, _msg: ExeFrontPlan, ctx: &mut Self::Context) -> Self::Result {
        // If paused or queue is empty, do nothing. This is a safeguard.
        if self.state.is_paused() || self.plan_queue.is_empty() {
            return Box::pin(async {}.into_actor(self));
        }

        // Pop the plan from the front of the queue to process it.
        let plan = self.plan_queue.pop_front().unwrap();
        self.stats.remain_plans_num -= 1;
        let address_pool = self.address_pool.clone();
        let account_generator = self.account_generator.clone();
        let monitor_addr = self.monitor_addr.clone();
        let consumer_addr = self.consumer_addr.clone();
        let self_addr = ctx.address();
        let sending_txns = self.stats.sending_txns.clone();
        let state = self.state.clone();
        let nonce_cache = self.nonce_cache.clone();
        Box::pin(
            async move {
                // Check if the plan is ready to be executed.
                let is_ready = Self::check_plan_ready(plan.execution_mode(), &address_pool).await;

                if !is_ready {
                    // If not ready, return the plan so it can be put back at the front of the queue.
                    tracing::debug!("Plan '{}' is not ready, re-queuing at front.", plan.name());
                    return Err(plan);
                }

                // If ready, execute the plan.
                let plan_id = plan.id().clone();
                if let Err(e) = Self::execute_plan(
                    monitor_addr,
                    consumer_addr,
                    address_pool,
                    account_generator,
                    plan,
                    sending_txns,
                    state,
                    nonce_cache,
                )
                .await
                {
                    tracing::error!("Execution of plan '{}' failed: {}", plan_id, e);
                    // Notify self of failure to handle cleanup and trigger the next plan.
                    self_addr.do_send(PlanFailed {
                        plan_id,
                        reason: e.to_string(),
                    });
                    return Ok(None);
                }

                // Return the plan_id for adding to awaiting_completion
                Ok(Some(plan_id))
            }
            .into_actor(self)
            .map(|result, act, ctx| {
                match result {
                    Err(plan) => {
                        // Plan was not ready, push it back to the front of the queue
                        act.stats.remain_plans_num += 1;
                        act.plan_queue.push_front(plan);
                        // Re-trigger plan execution after a short delay to avoid busy-looping.
                        ctx.run_later(Duration::from_millis(100), |act, ctx| {
                            act.trigger_next_plan_if_needed(ctx);
                        });
                    }
                    Ok(Some(plan_id)) => {
                        // Plan executed successfully, move to awaiting_completion
                        act.awaiting_completion.insert(plan_id);
                    }
                    Ok(None) => {
                        // Plan failed, already handled via PlanFailed message
                    }
                }
            }),
        )
    }
}

/// Handler for registering a new transaction plan.
impl Handler<RegisterTxnPlan> for Producer {
    type Result = ResponseFuture<Result<(), anyhow::Error>>;

    fn handle(&mut self, msg: RegisterTxnPlan, ctx: &mut Self::Context) -> Self::Result {
        // Only limit the queue size for plans waiting to be executed
        if self.plan_queue.len() >= self.max_queue_size {
            return Box::pin(async { Err(anyhow::anyhow!("Producer plan queue is full")) });
        }

        self.stats.remain_plans_num += 1;
        let plan_id = msg.plan.id().clone();
        tracing::debug!(
            "Registering new plan '{}' (id={}).",
            msg.plan.name(),
            plan_id
        );

        // Add the plan to the back of the queue.
        self.plan_queue.push_back(msg.plan);
        // Store the responder globally (will respond when plan completes)
        self.plan_responders.insert(plan_id, msg.responder);

        // A new plan has been added, so we attempt to trigger execution.
        // This is done synchronously within the handler to avoid race conditions.
        self.trigger_next_plan_if_needed(ctx);

        Box::pin(async { Ok(()) })
    }
}

/// Handler for successful plan completion notifications from the Monitor.
impl Handler<PlanCompleted> for Producer {
    type Result = ();

    fn handle(&mut self, msg: PlanCompleted, ctx: &mut Self::Context) {
        tracing::debug!(
            "Plan '{}' completed successfully and retain {} plans.",
            msg.plan_id,
            self.plan_queue.len()
        );
        self.stats.success_plans_num += 1;
        // Remove from awaiting_completion and respond to the original requester
        self.awaiting_completion.remove(&msg.plan_id);
        if let Some(responder) = self.plan_responders.remove(&msg.plan_id) {
            let _ = responder.send(Ok(()));
        }

        // The previous plan is done, so we attempt to trigger the next one.
        self.trigger_next_plan_if_needed(ctx);
    }
}

/// Handler for failed plan notifications.
impl Handler<PlanFailed> for Producer {
    type Result = ();

    fn handle(&mut self, msg: PlanFailed, ctx: &mut Self::Context) {
        tracing::error!(
            "Plan '{}' failed: {} and retain {} plans.",
            msg.plan_id,
            msg.reason,
            self.plan_queue.len()
        );
        self.stats.failed_plans_num += 1;
        // Remove from awaiting_completion and respond with error
        self.awaiting_completion.remove(&msg.plan_id);
        if let Some(responder) = self.plan_responders.remove(&msg.plan_id) {
            let _ = responder.send(Err(anyhow::anyhow!("Plan failed: {}", msg.reason)));
        }

        // Even if a plan failed, we attempt to trigger the next one in the queue.
        self.trigger_next_plan_if_needed(ctx);
    }
}

/// Handler for transaction submission results, which affects account readiness.
impl Handler<UpdateSubmissionResult> for Producer {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: UpdateSubmissionResult, _ctx: &mut Self::Context) -> Self::Result {
        let address_pool = self.address_pool.clone();
        self.stats
            .sending_txns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |val| {
                Some(val.saturating_sub(1))
            })
            .ok();
        match msg.result.as_ref() {
            SubmissionResult::Success(_) => {
                self.stats.success_txns += 1;
            }
            SubmissionResult::NonceTooLow { expect_nonce, .. } => {
                self.stats.success_txns += 1;
                let account_id = msg.metadata.from_account_id;
                self.nonce_cache.insert(account_id, *expect_nonce as u32);
            }
            SubmissionResult::ErrorWithRetry => {
                self.stats.failed_txns += 1;
            }
        }
        let ready_accounts = self.stats.ready_accounts.clone();
        Box::pin(
            async move {
                let account_id = msg.metadata.from_account_id;
                match msg.result.as_ref() {
                    SubmissionResult::Success(_) => {
                        address_pool.unlock_next_nonce(account_id);
                    }
                    SubmissionResult::NonceTooLow { expect_nonce, .. } => {
                        tracing::debug!(
                            "Nonce too low for account {:?}, expect nonce: {}, actual nonce: {}",
                            account_id,
                            expect_nonce,
                            msg.metadata.nonce
                        );
                        address_pool.unlock_correct_nonce(account_id, *expect_nonce as u32);
                    }
                    SubmissionResult::ErrorWithRetry => {
                        address_pool.retry_current_nonce(account_id);
                    }
                }
                ready_accounts.store(address_pool.ready_len() as u64, Ordering::Relaxed);
            }
            .into_actor(self)
            .map(|_, act, ctx| {
                // An account's nonce status has changed, which might make a pending plan ready.
                // We attempt to trigger the next plan.
                act.trigger_next_plan_if_needed(ctx);
            }),
        )
    }
}

/// Handler to pause the producer.
impl Handler<PauseProducer> for Producer {
    type Result = ();

    fn handle(&mut self, _msg: PauseProducer, _ctx: &mut Self::Context) {
        if self.state.is_running() {
            tracing::info!(
                "Producer paused. No new plans will be executed because the mempool is full."
            );
        }
        self.state.set_paused();
    }
}

/// Handler to resume the producer.
impl Handler<ResumeProducer> for Producer {
    type Result = ();

    fn handle(&mut self, _msg: ResumeProducer, ctx: &mut Self::Context) {
        if self.state.is_paused() {
            tracing::info!("Producer resumed.");
        }
        self.state.set_running();

        // The producer is running again, so we attempt to trigger a plan if one is waiting.
        self.trigger_next_plan_if_needed(ctx);
    }
}
