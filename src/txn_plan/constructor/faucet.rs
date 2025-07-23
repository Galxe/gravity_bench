use crate::{
    txn_plan::{faucet_plan::LevelFaucetPlan, traits::TxnPlan},
    util::gen_account,
};
use alloy::{
    primitives::{Address, U256},
    signers::local::PrivateKeySigner,
};
use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc, Mutex},
};
use tracing::info;

const FAUCET_DEGREE: usize = 10;
// Gas parameters must match the values used in the plan executor.
const GAS_LIMIT: u64 = 100_000;
const GAS_PRICE: u64 = 10_000_000_000; // 10 Gwei

pub struct FaucetTreePlanBuilder {
    faucet: Arc<PrivateKeySigner>,
    account_levels: Vec<Vec<Arc<PrivateKeySigner>>>,
    final_recipients: Arc<Vec<Arc<Address>>>,
    amount_per_recipient: U256,
    nonce_map: Arc<Mutex<HashMap<Address, Arc<AtomicU64>>>>,
    intermediate_funding_amounts: Vec<U256>,
    degree: usize,
    total_levels: usize,
}

impl FaucetTreePlanBuilder {
    pub fn new(
        faucet_balance: U256,
        faucet: PrivateKeySigner,
        start_nonce: u64,
        final_recipients: Arc<Vec<Arc<Address>>>,
    ) -> Self {
        let degree = FAUCET_DEGREE;
        let total_accounts = final_recipients.len();
        // total_levels represents the number of transfer stages.
        // e.g., total_levels = 2 means: Faucet -> L0 -> Final Recipients
        // This requires 1 intermediate account layer.
        let total_levels = Self::calculate_levels(total_accounts, degree);

        let degree_u256 = U256::from(degree);
        let gas_cost_per_txn = U256::from(GAS_LIMIT) * U256::from(GAS_PRICE);

        let (amount_per_recipient, intermediate_funding_amounts) = if total_levels > 1 {
            // This is a multi-level distribution.
            let num_intermediate_levels = total_levels - 1;

            let mut intermediate_txns: usize = 0;
            for i in 0..num_intermediate_levels {
                intermediate_txns += degree.pow(i as u32 + 1);
            }
            let final_txns = total_accounts;
            let total_txns = intermediate_txns + final_txns;
            let total_gas_cost = U256::from(total_txns) * gas_cost_per_txn;

            let amount_for_leaves = if faucet_balance > total_gas_cost {
                faucet_balance - total_gas_cost
            } else {
                U256::ZERO
            };

            let amount_per_recipient = if total_accounts > 0 {
                amount_for_leaves / U256::from(total_accounts)
            } else {
                U256::ZERO
            };

            let num_intermediate_levels = total_levels - 1;
            let mut intermediate_funding_amounts = vec![U256::ZERO; num_intermediate_levels];

            // Amount for the last intermediate level to send to final recipients.
            intermediate_funding_amounts[num_intermediate_levels - 1] =
                degree_u256 * (amount_per_recipient + gas_cost_per_txn);

            // Work backwards to calculate funding for previous levels.
            for i in (0..num_intermediate_levels - 1).rev() {
                intermediate_funding_amounts[i] =
                    degree_u256 * (intermediate_funding_amounts[i + 1] + gas_cost_per_txn);
            }
            (amount_per_recipient, intermediate_funding_amounts)
        } else {
            // No intermediate levels needed, direct distribution from faucet.
            let total_gas_cost = U256::from(total_accounts) * gas_cost_per_txn;
            let amount_for_leaves = if faucet_balance > total_gas_cost {
                faucet_balance - total_gas_cost
            } else {
                U256::ZERO
            };
            let amount_per_recipient = if total_accounts > 0 {
                amount_for_leaves / U256::from(total_accounts)
            } else {
                U256::ZERO
            };
            (amount_per_recipient, Vec::new())
        };

        let mut account_levels = vec![];
        if total_levels > 1 {
            let num_intermediate_levels = total_levels - 1;
            for level in 0..num_intermediate_levels {
                let num_accounts_at_level = degree.pow(level as u32 + 1);
                let accounts = gen_account::gen_account(num_accounts_at_level).unwrap();
                account_levels.push(accounts.values().map(|k| k.clone()).collect::<Vec<_>>());
            }
        }

        let mut nonce_map = HashMap::new();
        nonce_map.insert(faucet.address(), Arc::new(AtomicU64::new(start_nonce)));
        for level in &account_levels {
            for acc in level {
                nonce_map.insert(acc.address(), Arc::new(AtomicU64::new(0)));
            }
        }
        info!("FaucetTreePlanBuilder: balance={:?}, amount_per_recipient={:?}, intermediate_funding_amounts={:?}, accounts_levels={:?}, accounts_num={:?}", faucet_balance, amount_per_recipient, intermediate_funding_amounts, account_levels.len(), total_accounts);
        Self {
            faucet: Arc::new(faucet),
            account_levels,
            final_recipients,
            amount_per_recipient,
            nonce_map: Arc::new(Mutex::new(nonce_map)),
            intermediate_funding_amounts,
            degree,
            total_levels,
        }
    }

    pub fn total_levels(&self) -> usize {
        self.total_levels
    }

    fn calculate_levels(total_accounts: usize, degree: usize) -> usize {
        if total_accounts == 0 {
            0
        } else {
            // Calculate how many levels of distribution are needed.
            let mut levels = 1;
            let mut capacity = degree;
            while capacity < total_accounts {
                // Use checked_mul to prevent overflow, though it's unlikely with usize.
                if let Some(new_capacity) = capacity.checked_mul(degree) {
                    capacity = new_capacity;
                } else {
                    // Overflow means we have more than enough capacity.
                    levels += 1;
                    break;
                }
                levels += 1;
            }
            levels
        }
    }

    fn get_senders_for_level(&self, level: usize) -> Vec<Arc<PrivateKeySigner>> {
        if level == 0 {
            vec![self.faucet.clone()]
        } else {
            self.account_levels[level - 1].clone()
        }
    }

    pub fn create_plan_for_level(
        self: &Arc<Self>,
        level: usize,
        chain_id: u64,
    ) -> Box<dyn TxnPlan> {
        let senders = self.get_senders_for_level(level);
        let is_final_level = level == self.total_levels.saturating_sub(1);

        let plan = LevelFaucetPlan::new(
            chain_id,
            level,
            senders,
            self.final_recipients.clone(),
            self.account_levels.clone(),
            self.amount_per_recipient,
            self.intermediate_funding_amounts.clone(),
            self.degree,
            self.nonce_map.clone(),
            is_final_level,
        );
        Box::new(plan)
    }
}
