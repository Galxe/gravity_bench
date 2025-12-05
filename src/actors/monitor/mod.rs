mod mempool_tracker;

pub mod monitor_actor;
mod txn_tracker;

use actix::Message;
use alloy::primitives::{Address, TxHash};
use std::{sync::Arc, time::Instant};

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
    pub plan_name: String,
}

#[derive(Debug)]
pub enum SubmissionResult {
    NonceTooLow {
        tx_hash: TxHash,
        expect_nonce: u64,
        actual_nonce: u64,
        from_account: Arc<Address>,
    },
    ErrorWithRetry,
    Success(TxHash),
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct UpdateSubmissionResult {
    pub metadata: Arc<TxnMetadata>,
    pub result: Arc<SubmissionResult>,
    pub rpc_url: String,
    #[allow(unused)]
    pub send_time: Instant,
    // Added: raw transaction bytes for retry
    pub raw_tx: Option<Arc<Vec<u8>>>,
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

#[derive(Message)]
#[rtype(result = "()")]
pub struct RetryDroppedTxn {
    pub raw_tx: Arc<Vec<u8>>,
    pub metadata: Arc<TxnMetadata>,
    pub original_hash: TxHash,
}

pub use monitor_actor::Monitor;
