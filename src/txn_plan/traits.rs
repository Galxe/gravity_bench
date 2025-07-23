use std::sync::Arc;

use actix::Message;
use alloy::consensus::TxEnvelope;
use alloy::primitives::Address;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use uuid::Uuid;

#[derive(Debug, Clone, Message)]
#[rtype(result = "anyhow::Result<()>")]
pub struct SignedTxnWithMetadata {
    pub bytes: Vec<u8>,
    pub metadata: Arc<TxnMetadata>,
}

/// Metadata for a single transaction, linking it to its parent plan.
#[derive(Debug, Clone)]
pub struct TxnMetadata {
    pub txn_id: Uuid,
    pub plan_id: PlanId,
    pub from_account: Arc<Address>,
    pub nonce: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlanId {
    FullCheck(Uuid),
    PartialCheck(Uuid),
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanId::FullCheck(id) => write!(f, "FullCheck({})", id),
            PlanId::PartialCheck(id) => write!(f, "PartialCheck({})", id),
        }
    }
}

/// Execution mode: Determines the strategy for checking account readiness status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanExecutionMode {
    Full,           // Requires all accounts in AccountManager to be ready
    Partial(usize), // Can execute with partially ready accounts, at least usize number
}

pub trait FromTxnConstructor: Send + Sync + 'static {
    /// Build transaction based on sender information.
    /// The `to` address is usually a field of the constructor itself (e.g., fixed spender or router address).
    fn build_for_sender(
        &self,
        from_account: &Arc<Address>,
        from_signer: &Arc<PrivateKeySigner>,
        nonce: u64,
    ) -> Result<TransactionRequest, anyhow::Error>;

    /// Provide transaction description.
    fn description(&self) -> &'static str;
}

pub trait ToTxnConstructor: Send + Sync + 'static {
    /// Build transaction based on receiver information.
    fn build_for_receiver(
        &self,
        to: &Arc<Address>,
        chain_id: u64,
    ) -> Result<Vec<TxEnvelope>, anyhow::Error>;

    /// Provide transaction description.
    fn description(&self) -> &'static str;
}

pub struct TxnIter {
    pub iterator: crossbeam::channel::Receiver<SignedTxnWithMetadata>,
    pub consume_nonce: bool,
}

/// Defines a blueprint for a set of transactions.
///
/// A `TxnPlan` is an atomic, executable unit of work that generates one or more transactions.
/// It is responsible for reading the `AccountManager`'s state (e.g., ready accounts and nonces)
/// and generating a series of transactions based on its internal logic.
pub trait TxnPlan: Send + Sync {
    /// Builds and signs a vector of transactions based on the plan's logic.
    ///
    /// This method should read from the `AccountManager` to get available accounts and their nonces,
    /// but it should NOT mutate the manager. All state changes will be handled by the Producer
    /// after the transactions are built.
    fn build_txns(
        &mut self,
        ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
    ) -> Result<TxnIter, anyhow::Error>;

    /// Returns the unique identifier for this plan instance.
    fn id(&self) -> &PlanId;

    fn name(&self) -> &str;

    fn execution_mode(&self) -> &PlanExecutionMode;

    fn size(&self) -> Option<usize>;
}
