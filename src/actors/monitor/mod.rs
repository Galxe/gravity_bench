mod mempool_tracker;

pub mod monitor_actor;
mod txn_tracker;

use actix::Message;
use alloy::primitives::TxHash;
use std::sync::Arc;

use crate::txn_plan::{PlanId, TxnMetadata};

// Monitor Messages
#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterProducer {
    pub addr: actix::Addr<crate::actors::producer::Producer>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterConsumer {
    pub addr: actix::Addr<crate::actors::consumer::Consumer>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct RegisterPlan {
    pub plan_id: PlanId,
}

#[derive(Debug)]
pub enum SubmissionResult {
    NonceTooLow((u64, TxHash)),
    ErrorWithRetry,
    Success(TxHash),
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct UpdateSubmissionResult {
    pub metadata: Arc<TxnMetadata>,
    pub result: Arc<SubmissionResult>,
    pub rpc_url: String,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Tick;

// Monitor Messages
#[derive(Message)]
#[rtype(result = "()")]
pub struct PlanCompleted {
    pub plan_id: PlanId,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PlanFailed {
    pub plan_id: PlanId,
    pub reason: String,
}

pub use monitor_actor::Monitor;
