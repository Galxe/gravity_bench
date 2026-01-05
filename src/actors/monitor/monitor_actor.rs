use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use actix::{Actor, ActorFutureExt, Addr, AsyncContext, Context, Handler, Message, WrapFuture};
use futures::future;
use tracing::{error, info};

use crate::actors::consumer::Consumer;
use crate::actors::monitor::mempool_tracker::MempoolTracker;
use crate::actors::producer::Producer;
use crate::eth::EthHttpCli;
use crate::txn_plan::PlanId;

use super::txn_tracker::{BackpressureAction, PlanStatus, TxnTracker};
use super::mempool_tracker::MempoolAction;
use super::{
    PlanCompleted, PlanFailed, RegisterConsumer, RegisterPlan, RegisterProducer,
    ReportProducerStats, RetryTxn, Tick, UpdateSubmissionResult, CorrectNonces,
};
use crate::actors::{PauseProducer, ResumeProducer};

use crate::config::SamplingPolicy;

#[derive(Message)]
#[rtype(result = "()")]
struct LogStats;

/// Monitor Actor - Core for system state tracking and fault tolerance
pub struct Monitor {
    /// Registered Producer address
    producer_addr: Option<Addr<Producer>>,
    /// Registered Consumer address  
    consumer_addr: Option<Addr<Consumer>>,
    /// Transaction and plan tracker
    txn_tracker: TxnTracker,
    mempool_tracker: MempoolTracker,
    clients: Arc<HashMap<Arc<String>, Arc<EthHttpCli>>>,
}

impl Monitor {

    pub fn new_with_clients(
        clients: Vec<std::sync::Arc<EthHttpCli>>,
        max_pool_size: usize,
        sampling_policy: SamplingPolicy,
    ) -> Self {
        Self {
            producer_addr: None,
            consumer_addr: None,
            txn_tracker: TxnTracker::new(clients.clone(), sampling_policy),
            mempool_tracker: MempoolTracker::new(max_pool_size),
            clients: Arc::new(
                clients
                    .into_iter()
                    .map(|client| (client.rpc(), client))
                    .collect(),
            ),
        }
    }

    /// Notify Producer of plan status changes
    fn notify_plan_status(&self, plan_id: PlanId, status: PlanStatus) {
        if let Some(producer_addr) = &self.producer_addr {
            match status {
                PlanStatus::Completed => {
                    tracing::debug!("Plan {} completed successfully", plan_id);
                    producer_addr.do_send(PlanCompleted { plan_id });
                }
                PlanStatus::Failed { reason } => {
                    tracing::debug!("Plan {} failed: {}", plan_id, reason);
                    producer_addr.do_send(PlanFailed { plan_id, reason });
                }
                PlanStatus::InProgress => {
                    // Plan is still in progress, no need to notify
                }
            }
        }
    }
}

impl Actor for Monitor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("Monitor started");

        // Set up periodic Tick messages
        ctx.run_interval(Duration::from_secs(1), |_, ctx| {
            ctx.address().do_send(Tick);
        });
        ctx.run_interval(Duration::from_millis(1700), |_, ctx| {
            ctx.address().do_send(LogStats);
        });
        ctx.run_interval(Duration::from_secs(1), |act, ctx| {
            let client_clone = act.clients.clone();
            ctx.spawn(
                async move { MempoolTracker::get_pool_status(&client_clone).await }
                    .into_actor(act)
                    .map(|res, act, ctx| {
                        if let Some(producer_addr) = &act.producer_addr {
                            match act.mempool_tracker.process_pool_status(res, producer_addr) {
                                Ok((pending, queued, action)) => {
                                    act.txn_tracker.update_mempool_stats(pending, queued);
                                    
                                    // Handle nonce correction if needed
                                    if matches!(action, MempoolAction::NeedsNonceCorrection) {
                                        let clients = act.clients.clone();
                                        let producer = producer_addr.clone();
                                        ctx.spawn(
                                            async move {
                                                // Get the first client to fetch txpool_content
                                                if let Some(client) = clients.values().next() {
                                                    match client.get_txpool_content().await {
                                                        Ok(content) => {
                                                            let accounts = MempoolTracker::identify_problematic_accounts(&content);
                                                            if !accounts.is_empty() {
                                                                let mut corrections = Vec::new();
                                                                for account in accounts {
                                                                    match client.get_pending_txn_count(account).await {
                                                                        Ok(nonce) => {
                                                                            corrections.push(super::NonceCorrectionInfo {
                                                                                account,
                                                                                expected_nonce: nonce,
                                                                            });
                                                                        }
                                                                        Err(e) => {
                                                                            error!("Failed to get nonce for account {:?}: {}", account, e);
                                                                        }
                                                                    }
                                                                }
                                                                if !corrections.is_empty() {
                                                                    info!("Sending {} nonce corrections to producer", corrections.len());
                                                                    producer.do_send(CorrectNonces { corrections });
                                                                }
                                                            }
                                                        }
                                                        Err(e) => {
                                                            error!("Failed to get txpool_content: {}", e);
                                                        }
                                                    }
                                                }
                                            }
                                            .into_actor(act),
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to process pool status: {}", e);
                                }
                            }
                        }
                    }),
            );
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("Monitor stopped");
    }
}

impl Handler<RegisterProducer> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: RegisterProducer, _ctx: &mut Self::Context) {
        info!("Producer registered with Monitor");
        self.producer_addr = Some(msg.addr);
    }
}

impl Handler<RegisterConsumer> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: RegisterConsumer, _ctx: &mut Self::Context) {
        info!("Consumer registered with Monitor");
        self.consumer_addr = Some(msg.addr);
    }
}

impl Handler<RegisterPlan> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: RegisterPlan, _ctx: &mut Self::Context) {
        self.txn_tracker.register_plan(msg.plan_id, msg.plan_name);
    }
}

impl Handler<UpdateSubmissionResult> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: UpdateSubmissionResult, _ctx: &mut Self::Context) {
        // Forward message to producer
        if let Some(producer_addr) = &self.producer_addr {
            producer_addr.do_send(msg.clone());
        }

        match msg.result.as_ref() {
            crate::actors::monitor::SubmissionResult::ErrorWithRetry => {
                // If the transaction failed submission, retry it endlessly to prevent nonce gaps
                // and premature plan completion. Do NOT tell TxnTracker about the failure yet.
                tracing::warn!(
                    "Transaction failed submission (ErrorWithRetry). Retrying via Consumer. plan_id={}, tx_hash={:?}", 
                    msg.metadata.plan_id,
                    msg.metadata.txn_id
                );
                
                if let Some(consumer) = &self.consumer_addr {
                    consumer.do_send(RetryTxn {
                        signed_bytes: msg.signed_bytes.clone(),
                        metadata: msg.metadata.clone(),
                    });
                } else {
                    tracing::error!("Cannot retry transaction, no consumer address: {:?}", msg.metadata.txn_id);
                    // Fallback to tracker if no consumer (will mark as failed)
                    self.txn_tracker.handle_submission_result(&msg);
                }
            }
            _ => {
                 self.txn_tracker.handle_submission_result(&msg);
            }
        }
    }
}

impl Handler<Tick> for Monitor {
    type Result = ();

    fn handle(&mut self, _msg: Tick, ctx: &mut Self::Context) {
        // 1. Check local pending backpressure
        match self.txn_tracker.check_pending_backpressure() {
            BackpressureAction::Pause => {
                if let Some(producer_addr) = &self.producer_addr {
                    producer_addr.do_send(PauseProducer);
                }
            }
            BackpressureAction::Resume => {
                if let Some(producer_addr) = &self.producer_addr {
                    producer_addr.do_send(ResumeProducer);
                }
            }
            BackpressureAction::None => {}
        }

        // 2. Perform sampling check
        let tasks = self.txn_tracker.perform_sampling_check();
        let consumer_addr = self.consumer_addr.clone();
        if !tasks.is_empty() {
            ctx.spawn(
                future::join_all(tasks)
                    .into_actor(self)
                    .map(move |results, act, _ctx| {
                        // Process results and get retry queue
                        let retry_queue = act.txn_tracker.handle_receipt_result(results);

                        // 3. Send retries to consumer
                        if let Some(consumer) = &consumer_addr {
                            for retry_txn in retry_queue {
                                consumer.do_send(RetryTxn {
                                    signed_bytes: retry_txn.signed_bytes,
                                    metadata: retry_txn.metadata,
                                });
                            }
                        }
                    }),
            );
        }

        // Check completion status of all plans
        let plan_ids = self.txn_tracker.get_active_plan_ids();
        for plan_id in plan_ids {
            let status = self.txn_tracker.check_plan_completion(&plan_id);
            self.notify_plan_status(plan_id, status);
        }
    }
}

impl Handler<LogStats> for Monitor {
    type Result = ();

    fn handle(&mut self, _msg: LogStats, _ctx: &mut Self::Context) {
        self.txn_tracker.log_stats();
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct ProduceTxns {
    pub count: usize,
    pub plan_id: PlanId,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PlanProduced {
    pub plan_id: PlanId,
    pub count: usize,
}

impl Handler<ProduceTxns> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: ProduceTxns, _ctx: &mut Self::Context) {
        self.txn_tracker
            .handler_produce_txns(msg.plan_id, msg.count);
    }
}

impl Handler<PlanProduced> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: PlanProduced, _ctx: &mut Self::Context) {
        self.txn_tracker
            .handle_plan_produced(msg.plan_id, msg.count);
    }
}

impl Handler<ReportProducerStats> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: ReportProducerStats, _ctx: &mut Self::Context) {
        self.txn_tracker
            .update_producer_stats(msg.ready_accounts, msg.sending_txns);
    }
}

impl Handler<PlanFailed> for Monitor {
    type Result = ();

    fn handle(&mut self, msg: PlanFailed, _ctx: &mut Self::Context) {
        tracing::warn!("Plan {} failed: {}", msg.plan_id, msg.reason);
        self.txn_tracker.mark_plan_failed(msg.plan_id);
    }
}
