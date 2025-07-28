use std::sync::Arc;

use alloy::{
    primitives::{Address, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};

use crate::{eth::TxnBuilder, txn_plan::traits::FromTxnConstructor};

/// Token distribute constructor
/// Distribute tokens to accounts using ETH
#[derive(Clone)]
pub struct SwapEthToTokenConstructor {
    pub router_address: Address,
    pub token_address: Address,
    pub weth_address: Address,
    pub eth_amount_per_account: U256,
    pub amount_out_min: U256,
    pub chain_id: u64,
}

impl SwapEthToTokenConstructor {
    pub fn new(
        router_address: Address,
        token_address: Address,
        weth_address: Address,
        eth_amount_per_account: U256,
        amount_out_min: U256,
        chain_id: u64,
    ) -> Self {
        Self {
            router_address,
            token_address,
            weth_address,
            eth_amount_per_account,
            amount_out_min,
            chain_id,
        }
    }
}

impl FromTxnConstructor for SwapEthToTokenConstructor {
    fn build_for_sender(
        &self,
        _from_account: &Arc<Address>,
        from_signer: &Arc<PrivateKeySigner>,
        nonce: u64,
    ) -> Result<TransactionRequest, anyhow::Error> {
        // set transaction deadline (current time + 30 minutes)
        let deadline = U256::from(chrono::Utc::now().timestamp() + 1800);

        // build swap path: WETH -> Token
        let path = vec![self.weth_address, self.token_address];

        // build Uniswap V2 ETH for Token transaction request
        let tx_request = TxnBuilder::build_swap_exact_eth_for_tokens_request(
            self.router_address,
            self.amount_out_min,
            path,
            from_signer.address(),
            deadline,
            self.eth_amount_per_account,
            nonce,
            self.chain_id,
        )?;

        Ok(tx_request)
    }

    fn description(&self) -> &'static str {
        "Swap ETH for tokens"
    }
}
