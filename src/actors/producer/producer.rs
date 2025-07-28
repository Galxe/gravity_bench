use actix::prelude::*;
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::actors::consumer::Consumer;
use crate::actors::monitor::monitor::ProduceTxns;
use crate::actors::monitor::{
    Monitor, PlanCompleted, PlanFailed, RegisterPlan, RegisterProducer, SubmissionResult,
    UpdateSubmissionResult,
};
use crate::actors::producer::account_mgr::AccountManager;
use crate::actors::{ExeFrontPlan, PauseProducer, ResumeProducer};
use crate::txn_plan::{PlanExecutionMode, PlanId, TxnPlan};

use super::messages::RegisterTxnPlan;

#[derive(PartialEq, Eq, Debug)]
pub enum State {
    Running,
    Paused,
}

pub struct ProducerStats {
    pub remain_plans_num: u64,
    pub success_plans_num: u64,
    pub failed_plans_num: u64,
    pub sending_txns: Arc<AtomicU64>,
    pub success_txns: u64,
    pub failed_txns: u64,
    pub ready_accounts: Arc<AtomicU64>,
}

/// The main TxnProducer actor, responsible for executing transaction plans sequentially.
pub struct Producer {
    /// Manages account nonces and readiness.
    account_manager: Arc<Mutex<AccountManager>>,
    /// The current state of the producer (Running or Paused).
    state: State,

    stats: ProducerStats,

    /// Actor addresses for communication with other parts of the system.
    monitor_addr: Addr<Monitor>,
    consumer_addr: Addr<Consumer>,

    /// A queue of plans waiting to be executed. Plans are processed in FIFO order.
    plan_queue: VecDeque<Box<dyn TxnPlan>>,
    /// Tracks pending plans to respond to the original requester upon completion.
    pending_plans: HashMap<PlanId, tokio::sync::oneshot::Sender<Result<(), anyhow::Error>>>,
    /// The maximum number of plans allowed in the queue.
    max_queue_size: usize,
}

impl Producer {
    pub fn new(
        account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>,
        consumer_addr: Addr<Consumer>,
        monitor_addr: Addr<Monitor>,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            state: State::Running,
            stats: ProducerStats {
                remain_plans_num: 0,
                sending_txns: Arc::new(AtomicU64::new(0)),
                ready_accounts: Arc::new(AtomicU64::new(0)),
                success_plans_num: 0,
                failed_plans_num: 0,
                success_txns: 0,
                failed_txns: 0,
            },
            account_manager: Arc::new(Mutex::new(AccountManager::new(account_signers))),
            monitor_addr,
            consumer_addr,
            plan_queue: VecDeque::new(),
            pending_plans: HashMap::new(),
            max_queue_size: 100000,
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
        if self.state == State::Running && !self.plan_queue.is_empty() {
            ctx.address().do_send(ExeFrontPlan);
        }
    }

    /// Checks if the required accounts for a given plan are ready.
    async fn check_plan_ready(
        plan_mode: &PlanExecutionMode,
        account_manager: &Arc<Mutex<AccountManager>>,
    ) -> bool {
        let manager = account_manager.lock().await;
        match plan_mode {
            PlanExecutionMode::Full => manager.is_full_ready(),
            PlanExecutionMode::Partial(required_count) => manager.ready_len() >= *required_count,
        }
    }

    /// The core logic for executing a single transaction plan.
    /// This is a static method to make dependencies explicit.
    async fn execute_plan(
        monitor_addr: Addr<Monitor>,
        consumer_addr: Addr<Consumer>,
        account_manager: Arc<Mutex<AccountManager>>,
        mut plan: Box<dyn TxnPlan>,
        sending_txns: Arc<AtomicU64>,
    ) -> Result<(), anyhow::Error> {
        tracing::info!("Starting execution of plan: {}", plan.name());
        let plan_id = plan.id().clone();

        // Fetch accounts and build transactions
        let ready_accounts = {
            let mut manager = account_manager.lock().await;
            manager.fetch_ready_accounts(plan.size())
        }; // Lock is released here

        let iterator = plan.as_mut().build_txns(ready_accounts)?;

        // If the plan doesn't consume nonces, accounts can be used by other processes immediately.
        if !iterator.consume_nonce {
            let mut manager = account_manager.lock().await;
            manager.resume_all_accounts();
        }
        // must send to monitor before sending to consumer
        monitor_addr
            .send(RegisterPlan {
                plan_id: plan_id.clone(),
            })
            .await
            .unwrap();
        let mut count = 0;
        // Send all signed transactions to the consumer
        while let Ok(signed_txn) = iterator.iterator.recv() {
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

        tracing::info!(
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
        let account_manager = self.account_manager.clone();
        async move {
            let count = account_manager.lock().await.len();
            tracing::info!("Producer started with {} accounts.", count);
        }
        .into_actor(self)
        .wait(ctx);
        ctx.run_interval(Duration::from_secs(5), |act, _ctx| {
            tracing::info!("Producer stats: plans_num={}, sending_txns={}, ready_accounts={}, success_plans_num={}, failed_plans_num={}, success_txns={}, failed_txns={}", act.stats.remain_plans_num, act.stats.sending_txns.load(Ordering::Relaxed), act.stats.ready_accounts.load(Ordering::Relaxed), act.stats.success_plans_num, act.stats.failed_plans_num, act.stats.success_txns, act.stats.failed_txns);
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
        if self.state == State::Paused || self.plan_queue.is_empty() {
            return Box::pin(async {}.into_actor(self));
        }

        // Pop the plan from the front of the queue to process it.
        let plan = self.plan_queue.pop_front().unwrap();
        self.stats.remain_plans_num -= 1;
        let account_manager = self.account_manager.clone();
        let monitor_addr = self.monitor_addr.clone();
        let consumer_addr = self.consumer_addr.clone();
        let self_addr = ctx.address();
        let sending_txns = self.stats.sending_txns.clone();
        Box::pin(
            async move {
                // Check if the plan is ready to be executed.
                let is_ready =
                    Self::check_plan_ready(plan.execution_mode(), &account_manager).await;

                if !is_ready {
                    // If not ready, return the plan so it can be put back at the front of the queue.
                    tracing::debug!("Plan '{}' is not ready, re-queuing at front.", plan.name());
                    return Some(plan);
                }

                // If ready, execute the plan.
                let plan_id = plan.id().clone();
                if let Err(e) = Self::execute_plan(
                    monitor_addr,
                    consumer_addr,
                    account_manager,
                    plan,
                    sending_txns,
                )
                .await
                {
                    tracing::error!("Execution of plan '{}' failed: {}", plan_id, e);
                    // Notify self of failure to handle cleanup and trigger the next plan.
                    self_addr.do_send(PlanFailed {
                        plan_id,
                        reason: e.to_string(),
                    });
                }

                // Return None as the plan has been consumed (either successfully started or failed).
                None
            }
            .into_actor(self)
            .map(|maybe_plan, act, _ctx| {
                // This block runs after the async block is complete, safely on the actor's context.
                if let Some(plan) = maybe_plan {
                    act.stats.remain_plans_num += 1;
                    // CRITICAL: If the plan was not ready, push it back to the *front* of the queue
                    // to ensure strict sequential execution. No other plan can run before it.
                    act.plan_queue.push_front(plan);
                }
            }),
        )
    }
}

/// Handler for registering a new transaction plan.
impl Handler<RegisterTxnPlan> for Producer {
    type Result = ResponseFuture<Result<(), anyhow::Error>>;

    fn handle(&mut self, msg: RegisterTxnPlan, ctx: &mut Self::Context) -> Self::Result {
        if self.pending_plans.len() >= self.max_queue_size {
            return Box::pin(async { Err(anyhow::anyhow!("Producer plan queue is full")) });
        }

        self.stats.remain_plans_num += 1;
        let plan_id = msg.plan.id().clone();
        tracing::info!(
            "Registering new plan '{}' (id={}).",
            msg.plan.name(),
            plan_id
        );

        // Add the plan to the back of the queue.
        self.plan_queue.push_back(msg.plan);
        self.pending_plans.insert(plan_id, msg.responder);

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
        tracing::info!(
            "Plan '{}' completed successfully and retain {} plans.",
            msg.plan_id,
            self.plan_queue.len()
        );
        self.stats.success_plans_num += 1;
        if let Some(responder) = self.pending_plans.remove(&msg.plan_id) {
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

        if let Some(responder) = self.pending_plans.remove(&msg.plan_id) {
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
        let account_manager = self.account_manager.clone();
        let account = msg.metadata.from_account.clone();
        self.stats.sending_txns.fetch_sub(1, Ordering::Relaxed);
        match msg.result.as_ref() {
            SubmissionResult::Success(_) => {
                self.stats.success_txns += 1;
            }
            SubmissionResult::NonceTooLow(_) => {
                self.stats.failed_txns += 1;
            }
            SubmissionResult::ErrorWithRetry => {
                self.stats.failed_txns += 1;
            }
        }
        let ready_accounts = self.stats.ready_accounts.clone();
        Box::pin(
            async move {
                let mut manager = account_manager.lock().await;
                match msg.result.as_ref() {
                    SubmissionResult::Success(_) => {
                        manager.unlock_next_nonce(account);
                    }
                    SubmissionResult::NonceTooLow((nonce, _tx_hash)) => {
                        manager.unlock_correct_nonce(account, *nonce as u32);
                    }
                    SubmissionResult::ErrorWithRetry => {
                        manager.retry_current_nonce(account);
                    }
                }
                ready_accounts.store(manager.ready_len() as u64, Ordering::Relaxed);
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
        tracing::info!("Producer paused. No new plans will be executed.");
        self.state = State::Paused;
    }
}

/// Handler to resume the producer.
impl Handler<ResumeProducer> for Producer {
    type Result = ();

    fn handle(&mut self, _msg: ResumeProducer, ctx: &mut Self::Context) {
        tracing::info!("Producer resumed.");
        self.state = State::Running;

        // The producer is running again, so we attempt to trigger a plan if one is waiting.
        self.trigger_next_plan_if_needed(ctx);
    }
}
