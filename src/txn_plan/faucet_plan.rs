use crate::{
    eth::TxnBuilder,
    txn_plan::traits::{
        PlanExecutionMode, PlanId, SignedTxnWithMetadata, TxnMetadata, TxnPlan,
    },
};
use alloy::{
    eips::Encodable2718, network::TransactionBuilder, primitives::{Address, U256}, rpc::types::TransactionRequest, signers::local::PrivateKeySigner
};
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

use super::TxnIter;

const DEFAULT_CONCURRENCY_LIMIT: usize = 256;

pub struct LevelFaucetPlan {
    id: PlanId,
    execution_mode: PlanExecutionMode,
    chain_id: u64,
    level: usize,
    senders: Vec<Arc<PrivateKeySigner>>,
    final_recipients: Arc<Vec<Arc<Address>>>,
    account_levels: Vec<Vec<Arc<PrivateKeySigner>>>,
    amount_per_recipient: U256,
    intermediate_funding_amounts: Vec<U256>,
    degree: usize,
    nonce_map: Arc<Mutex<HashMap<Address, Arc<AtomicU64>>>>,
    is_final_level: bool,
    concurrency_limit: usize,
}

impl LevelFaucetPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: u64,
        level: usize,
        senders: Vec<Arc<PrivateKeySigner>>,
        final_recipients: Arc<Vec<Arc<Address>>>,
        account_levels: Vec<Vec<Arc<PrivateKeySigner>>>,
        amount_per_recipient: U256,
        intermediate_funding_amounts: Vec<U256>,
        degree: usize,
        nonce_map: Arc<Mutex<HashMap<Address, Arc<AtomicU64>>>>,
        is_final_level: bool,
    ) -> Self {
        let execution_mode = PlanExecutionMode::Full;
        let id = match execution_mode {
            PlanExecutionMode::Full => PlanId::FullCheck(Uuid::new_v4()),
            PlanExecutionMode::Partial(_) => PlanId::PartialCheck(Uuid::new_v4()),
        };
        Self {
            id,
            execution_mode,
            chain_id,
            level,
            senders,
            final_recipients,
            account_levels,
            amount_per_recipient,
            intermediate_funding_amounts,
            degree,
            nonce_map,
            is_final_level,
            concurrency_limit: DEFAULT_CONCURRENCY_LIMIT,
        }
    }
}

#[async_trait::async_trait]
impl TxnPlan for LevelFaucetPlan {
    fn id(&self) -> &PlanId {
        &self.id
    }

    fn size(&self) -> Option<usize> {
        None
    }

    fn execution_mode(&self) -> &PlanExecutionMode {
        &self.execution_mode
    }

    fn name(&self) -> &str {
        "LevelFaucetPlan"
    }

    fn build_txns(
        &mut self,
        _ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
    ) -> Result<TxnIter, anyhow::Error> {
        let plan_id = self.id.clone();
        let (tx, rx) = crossbeam::channel::bounded(self.concurrency_limit);
        let senders = self.senders.clone();
        let degree = self.degree;
        let final_recipients = self.final_recipients.clone();
        let account_levels = self.account_levels.clone();
        let amount_per_recipient = self.amount_per_recipient;
        let intermediate_funding_amounts = self.intermediate_funding_amounts.clone();
        let nonce_map = self.nonce_map.clone();
        let chain_id = self.chain_id;
        let level = self.level;
        let is_final_level = self.is_final_level;
        let handle = tokio::task::spawn_blocking(move || {
            senders
                .into_par_iter()
                .enumerate()
                .for_each(|(sender_index, sender_signer)| {
                    let start_index = sender_index * degree;
                    let end_index = (start_index + degree).min(final_recipients.len());

                    for i in start_index..end_index {
                        let (to_address, value) = if is_final_level {
                            let to = final_recipients[i].clone();
                            let val = amount_per_recipient;
                            (to, val)
                        } else {
                            let to = account_levels[level][i].address();
                            let val = intermediate_funding_amounts[level];
                            (Arc::new(to), val)
                        };

                        let nonce_map_guard = nonce_map.lock().unwrap();
                        let nonce = nonce_map_guard
                            .get(&sender_signer.address())
                            .unwrap()
                            .fetch_add(1, Ordering::Relaxed);

                        let tx_request = TransactionRequest::default()
                            .with_to(*to_address)
                            .with_value(value)
                            .with_nonce(nonce)
                            .with_chain_id(chain_id)
                            .with_max_priority_fee_per_gas(10_000_000_000) // 0.1 gwei
                            .with_max_fee_per_gas(10_000_000_000) // 0.1 gwei
                            .with_gas_limit(100_000); // Standard gas for ERC20 transfer
                        let tx_envelope =
                            TxnBuilder::build_and_sign_transaction(tx_request, &sender_signer)
                                .unwrap();

                        let metadata = Arc::new(TxnMetadata {
                            from_account: Arc::new(sender_signer.address()),
                            nonce,
                            txn_id: Uuid::new_v4(),
                            plan_id: plan_id.clone(),
                        });

                        tx.send(SignedTxnWithMetadata {
                            bytes: tx_envelope.encoded_2718(),
                            metadata,
                        })
                        .unwrap();
                    }
                });
            drop(tx);
        });
        tokio::spawn(async move {
            handle.await.unwrap();
        });

        Ok(TxnIter {
            iterator: rx,
            consume_nonce: false,
        })
    }
} 