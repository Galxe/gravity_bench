use crate::txn_plan::TxnPlan;
use actix::prelude::*;

/// Commands from the Engine to control the TxnProducer
#[derive(Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct RegisterTxnPlan {
    pub plan: Box<dyn TxnPlan>,
    pub responder: tokio::sync::oneshot::Sender<Result<(), anyhow::Error>>,
}

impl RegisterTxnPlan {
    pub fn new(
        plan: Box<dyn TxnPlan>,
    ) -> (
        Self,
        tokio::sync::oneshot::Receiver<Result<(), anyhow::Error>>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            Self {
                plan,
                responder: tx,
            },
            rx,
        )
    }
}

#[derive(Message, Default)]
#[rtype(result = "()")]
pub struct ExeFrontPlan;

#[derive(Message, Default)]
#[rtype(result = "()")]
pub struct PauseProducer;

#[derive(Message, Default)]
#[rtype(result = "()")]
pub struct ResumeProducer;
