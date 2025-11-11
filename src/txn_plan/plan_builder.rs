use std::{str::FromStr, sync::Arc};

use alloy::{
    primitives::{Address, U256},
    signers::local::PrivateKeySigner,
};

use crate::{
    config::LiquidityPair, eth::EthHttpCli, txn_plan::{
        TxnPlan, constructor::{
            ApproveTokenConstructor, BalanceFetcher, Erc20TransferConstructor, FaucetTreePlanBuilder, SwapEthToTokenConstructor, SwapTokenToTokenConstructor
        }, faucet_txn_builder::FaucetTxnBuilder, plan::ManyToOnePlan, traits::PlanExecutionMode
    }
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
        address_list: Arc<Vec<Arc<Address>>>,
        router_address: Address,
        size: usize,
    ) -> Box<dyn TxnPlan> {
        let constructor = SwapTokenToTokenConstructor::new(
            token_list,
            chain_id,
            address_list,
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
        eth_client: BalanceFetcher,
        faucet_private_key: &str,
        faucet_start_nonce: u64,
        total_accounts: Arc<Vec<Arc<Address>>>,
        txn_builder: Arc<T>,
        remained_eth: U256,
        account_generator: &mut crate::util::gen_account::AccountGenerator,
    ) -> Result<Arc<FaucetTreePlanBuilder<T>>, anyhow::Error> {
        let faucet_signer = PrivateKeySigner::from_str(faucet_private_key)?;
        let constructor = FaucetTreePlanBuilder::new(
            eth_client,
            faucet_level,
            faucet_signer,
            faucet_start_nonce,
            total_accounts,
            txn_builder,
            remained_eth,
            account_generator,
        ).await;
        Ok(Arc::new(constructor))
    }

    /// Create ERC20 token transfer plan
    pub fn erc20_transfer(
        chain_id: u64,
        token_list: Vec<Address>,
        transfer_amount: U256,
        address_list: Arc<Vec<Arc<Address>>>,
        size: usize,
    ) -> Box<dyn TxnPlan> {
        let constructor =
            Erc20TransferConstructor::new(token_list, transfer_amount, chain_id, address_list);
        let plan = ManyToOnePlan::new(constructor, PlanExecutionMode::Partial(size));
        let plan = plan.with_size(size);
        Box::new(plan)
    }
}
