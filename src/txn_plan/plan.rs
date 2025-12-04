use crate::{
    eth::TxnBuilder,
    txn_plan::{
        traits::{
            FromTxnConstructor, PlanExecutionMode, PlanId, SignedTxnWithMetadata, ToTxnConstructor,
            TxnMetadata, TxnPlan,
        },
        TxnIter,
    },
    util::gen_account::{AccountId, AccountManager},
};
use alloy::{consensus::transaction::SignerRecoverable, eips::Encodable2718};
use rayon::iter::IntoParallelIterator;
use rayon::iter::ParallelIterator;
use std::sync::Arc;
use uuid::Uuid;

/// Concurrency control parameters
const DEFAULT_CONCURRENCY_LIMIT: usize = 50;

/// Generic Plan executor designed for `ManyToOneConstructor`.
pub struct ManyToOnePlan<C>
where
    C: FromTxnConstructor,
{
    size: Option<usize>,
    id: PlanId,
    execution_mode: PlanExecutionMode,
    constructor: Arc<C>,
    concurrency_limit: usize,
}

impl<C: FromTxnConstructor> ManyToOnePlan<C> {
    pub fn new(constructor: C, execution_mode: PlanExecutionMode) -> Self {
        let id = match execution_mode {
            PlanExecutionMode::Full => PlanId::FullCheck(Uuid::new_v4()),
            PlanExecutionMode::Partial(_) => PlanId::PartialCheck(Uuid::new_v4()),
        };

        Self {
            size: None,
            id,
            execution_mode,
            constructor: Arc::new(constructor),
            concurrency_limit: DEFAULT_CONCURRENCY_LIMIT,
        }
    }

    pub fn with_size(mut self, size: usize) -> Self {
        self.size = Some(size);
        self
    }
}

#[async_trait::async_trait]
impl<C: FromTxnConstructor> TxnPlan for ManyToOnePlan<C> {
    fn id(&self) -> &PlanId {
        &self.id
    }

    fn size(&self) -> Option<usize> {
        self.size
    }

    fn execution_mode(&self) -> &PlanExecutionMode {
        &self.execution_mode
    }

    fn name(&self) -> &str {
        self.constructor.description()
    }

    fn build_txns(
        &mut self,
        ready_accounts: Vec<(AccountId, u32)>,
        account_generator: AccountManager,
    ) -> Result<TxnIter, anyhow::Error> {
        let plan_id = self.id.clone();
        let constructor = self.constructor.clone();
        let (tx, rx) = crossbeam::channel::bounded(self.concurrency_limit);

        // 4. Create async stream, process in batches
        let handle = tokio::task::spawn_blocking(move || {
            ready_accounts
                .chunks(1024)
                .map(|chunk| {
                    chunk
                        .into_par_iter()
                        .map(|(from_account_id, nonce)| {
                            let address = account_generator.get_address_by_id(*from_account_id);
                            let signer =
                                account_generator.get_signer_by_id(*from_account_id).clone();
                            let tx_request = constructor
                                .build_for_sender(
                                    *from_account_id,
                                    account_generator.clone(),
                                    *nonce as u64,
                                )
                                .unwrap();
                            let metadata = Arc::new(TxnMetadata {
                                from_account: Arc::new(address),
                                nonce: *nonce as u64,
                                txn_id: Uuid::new_v4(),
                                plan_id: plan_id.clone(),
                            });
                            let tx_envelope =
                                TxnBuilder::build_and_sign_transaction(tx_request, &signer)
                                    .unwrap();
                            SignedTxnWithMetadata {
                                bytes: tx_envelope.encoded_2718(),
                                metadata,
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .for_each(|txns| {
                    for txn in txns {
                        tx.send(txn).unwrap();
                    }
                });
        });
        tokio::spawn(async move {
            handle.await.unwrap();
        });

        Ok(TxnIter {
            iterator: rx,
            consume_nonce: true,
        })
    }
}

/// Generic Plan executor designed for `OneToManyConstructor`.
pub struct OneToManyPlan<C>
where
    C: ToTxnConstructor,
{
    id: PlanId,
    execution_mode: PlanExecutionMode,
    chain_id: u64,
    constructor: Arc<C>,
    concurrency_limit: usize,
}

impl<C: ToTxnConstructor> OneToManyPlan<C> {
    #[allow(unused)]
    pub fn new(constructor: C, execution_mode: PlanExecutionMode, chain_id: u64) -> Self {
        let id = match execution_mode {
            PlanExecutionMode::Full => PlanId::FullCheck(Uuid::new_v4()),
            PlanExecutionMode::Partial(_) => PlanId::PartialCheck(Uuid::new_v4()),
        };

        Self {
            id,
            execution_mode,
            chain_id,
            constructor: Arc::new(constructor),
            concurrency_limit: DEFAULT_CONCURRENCY_LIMIT,
        }
    }
}

#[async_trait::async_trait]
impl<C: ToTxnConstructor> TxnPlan for OneToManyPlan<C> {
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
        self.constructor.description()
    }

    fn build_txns(
        &mut self,
        ready_accounts: Vec<(AccountId, u32)>,
        account_generator: AccountManager,
    ) -> Result<TxnIter, anyhow::Error> {
        // 3. Parallelly build and sign transactions
        let plan_id = self.id.clone();
        let constructor = self.constructor.clone();
        let chain_id = self.chain_id;
        let (tx, rx) = crossbeam::channel::bounded(self.concurrency_limit);

        // Convert AccountId to addresses
        let addresses = ready_accounts;

        let handle = tokio::task::spawn_blocking(move || {
            addresses
                .chunks(1024)
                .map(|chunk| {
                    chunk
                        .into_par_iter()
                        .flat_map(|(to_account_id, _nonce)| {
                            // Build transaction request
                            let txs = constructor
                                .build_for_receiver(
                                    *to_account_id,
                                    account_generator.clone(),
                                    chain_id,
                                )
                                .unwrap();
                            txs.into_iter()
                                .map(|tx_envelope| {
                                    let metadata = Arc::new(TxnMetadata {
                                        from_account: Arc::new(
                                            tx_envelope.recover_signer_unchecked().unwrap(),
                                        ),
                                        nonce: 0,
                                        txn_id: Uuid::new_v4(),
                                        plan_id: plan_id.clone(),
                                    });
                                    SignedTxnWithMetadata {
                                        bytes: tx_envelope.encoded_2718(),
                                        metadata,
                                    }
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>()
                })
                .for_each(|txns| {
                    for txn in txns {
                        tx.send(txn).unwrap();
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
