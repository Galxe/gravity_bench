use std::{str::FromStr, sync::Arc};

use alloy::{
    primitives::{Address, U256},
    signers::local::PrivateKeySigner,
};
use tokio::sync::RwLock;

use crate::{
    config::LiquidityPair,
    txn_plan::{
        addr_pool::AddressPool,
        constructor::{
            ApproveTokenConstructor, Erc20TransferConstructor, FaucetTreePlanBuilder,
            SwapEthToTokenConstructor, SwapTokenToTokenConstructor,
        },
        faucet_txn_builder::FaucetTxnBuilder,
        plan::ManyToOnePlan,
        traits::PlanExecutionMode,
        TxnPlan,
    },
    util::gen_account::AccountGenerator,
};

/// Plan builder - Provides convenient APIs to create various types of transaction plans
pub struct PlanBuilder;

impl PlanBuilder {
    /// Create ERC20 token approval plan
    pub fn approve_token(
        chain_id: u64,
        token_address: Address,
        spender_address: Address,
    ) -> Box<dyn TxnPlan> {
        let constructor = ApproveTokenConstructor::new(chain_id, token_address, spender_address);
        Box::new(ManyToOnePlan::new(constructor, PlanExecutionMode::Full))
    }

    pub fn swap_token_to_token(
        chain_id: u64,
        amount_in: U256,
        token_list: Vec<LiquidityPair>,
        address_pool: Arc<dyn AddressPool>,
        router_address: Address,
        size: usize,
    ) -> Box<dyn TxnPlan> {
        let constructor = SwapTokenToTokenConstructor::new(
            token_list,
            chain_id,
            address_pool,
            amount_in,
            router_address,
        );
        let plan = ManyToOnePlan::new(constructor, PlanExecutionMode::Partial(size));
        let plan = plan.with_size(size);
        Box::new(plan)
    }

    /// Create token distribution plan (swap tokens through Uniswap)
    #[allow(unused)]
    pub fn swap_eth_to_token(
        router_address: Address,
        token_address: Address,
        weth_address: Address,
        eth_amount_per_account: U256,
        amount_out_min: U256,
        chain_id: u64,
    ) -> Box<dyn TxnPlan> {
        let constructor = SwapEthToTokenConstructor::new(
            router_address,
            token_address,
            weth_address,
            eth_amount_per_account,
            amount_out_min,
            chain_id,
        );
        Box::new(ManyToOnePlan::new(constructor, PlanExecutionMode::Full))
    }

    /// Create ETH distribution plan
    pub async fn create_faucet_tree_plan_builder<T: FaucetTxnBuilder + 'static>(
        faucet_level: usize,
        faucet_balance: U256,
        faucet_private_key: &str,
        faucet_start_nonce: u64,
        total_accounts: Arc<Vec<Arc<Address>>>,
        txn_builder: Arc<T>,
        remained_eth: U256,
        account_generator: Arc<RwLock<AccountGenerator>>,
    ) -> Result<Arc<FaucetTreePlanBuilder<T>>, anyhow::Error> {
        let faucet_signer = PrivateKeySigner::from_str(faucet_private_key)?;
        let constructor = FaucetTreePlanBuilder::new(
            faucet_balance,
            faucet_level,
            faucet_signer,
            faucet_start_nonce,
            total_accounts,
            txn_builder,
            remained_eth,
            account_generator,
        )
        .await;
        Ok(Arc::new(constructor))
    }

    /// Create ERC20 token transfer plan
    pub fn erc20_transfer(
        chain_id: u64,
        token_list: Vec<Address>,
        transfer_amount: U256,
        address_pool: Arc<dyn AddressPool>,
        size: usize,
    ) -> Box<dyn TxnPlan> {
        let constructor =
            Erc20TransferConstructor::new(token_list, transfer_amount, chain_id, address_pool);
        let plan = ManyToOnePlan::new(constructor, PlanExecutionMode::Partial(size));
        let plan = plan.with_size(size);
        Box::new(plan)
    }
}
