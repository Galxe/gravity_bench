use crate::{
    config::{IUniswapV2Router, LiquidityPair},
    txn_plan::{FromTxnConstructor, addr_pool::AddressPool}, util::gen_account::{AccountGenerator, AccountId, AccountManager},
};
use alloy::{
    network::TransactionBuilder,
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use std::{str::FromStr, sync::Arc};

pub struct SwapTokenToTokenConstructor {
    pub token_list: Vec<LiquidityPair>,
    pub chain_id: u64,
    pub address_pool: Arc<dyn AddressPool>,
    pub transter_amount: U256,
    pub router_address: Address,
}

impl SwapTokenToTokenConstructor {
    pub fn new(
        token_list: Vec<LiquidityPair>,
        chain_id: u64,
        address_pool: Arc<dyn AddressPool>,
        transter_amount: U256,
        router_address: Address,
    ) -> Self {
        Self {
            token_list,
            chain_id,
            address_pool,
            transter_amount,
            router_address,
        }
    }
}

impl FromTxnConstructor for SwapTokenToTokenConstructor {
    fn build_for_sender(
        &self,
        from_account_id: AccountId,
        account_generator: AccountManager,
        nonce: u64,
    ) -> Result<TransactionRequest, anyhow::Error> {
        let to_address = self.address_pool.select_receiver(from_account_id);
        let to_address = account_generator.get_address_by_id(to_address);
        let from_address = account_generator.get_address_by_id(from_account_id);
        let token_idx = rand::random::<usize>() % self.token_list.len();
        let from_token = Address::from_str(&self.token_list[token_idx].token_a_address).unwrap();
        let to_token = Address::from_str(&self.token_list[token_idx].token_b_address).unwrap();
        let path = vec![from_token, to_token];
        let swap_call = IUniswapV2Router::swapExactTokensForTokensCall {
            amountIn: self.transter_amount,
            amountOutMin: U256::from(0),
            path,
            to: to_address,
            deadline: U256::from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 1000,
            ),
        };
        let call_data = swap_call.abi_encode();
        let call_data = Bytes::from(call_data);
        let tx_request = TransactionRequest::default()
            .with_from(from_address)
            .with_to(self.router_address)
            .with_input(call_data)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_max_priority_fee_per_gas(100_000_000) // 0.1 gwei
            .with_max_fee_per_gas(100_000_000) // 0.1 gwei
            .with_gas_limit(100_000); // Standard gas for ETH transfer
        Ok(tx_request)
    }

    fn description(&self) -> &'static str {
        "Swap token to token"
    }
}
