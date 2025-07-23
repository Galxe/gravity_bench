use std::sync::Arc;

use alloy::{
    network::TransactionBuilder,
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};

use crate::{config::IERC20, txn_plan::traits::FromTxnConstructor};

/// ERC20 approve constructor
/// Approve tokens for multiple accounts to a specified spender (e.g. Uniswap Router)
#[derive(Clone)]
pub struct ApproveTokenConstructor {
    pub token_address: Address,
    pub spender_address: Address,
    pub chain_id: u64,
}

impl ApproveTokenConstructor {
    pub fn new(chain_id: u64, token_address: Address, spender_address: Address) -> Self {
        Self {
            chain_id,
            token_address,
            spender_address,
        }
    }
}

impl FromTxnConstructor for ApproveTokenConstructor {
    fn build_for_sender(
        &self,
        from_account: &Arc<Address>,
        _from_signer: &Arc<PrivateKeySigner>,
        nonce: u64,
    ) -> Result<TransactionRequest, anyhow::Error> {
        let approve_call = IERC20::approveCall {
            spender: self.spender_address,
            amount: U256::MAX,
        };

        let call_data = approve_call.abi_encode();
        let call_data = Bytes::from(call_data);

        // create transaction request
        let tx_request = TransactionRequest::default()
            .with_from(*from_account.as_ref())
            .with_to(self.token_address)
            .with_input(call_data)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_max_priority_fee_per_gas(10_000_000_000) // 0.1 gwei
            .with_max_fee_per_gas(10_000_000_000) // 0.1 gwei
            .with_gas_limit(100_000); // Standard gas for approve

        Ok(tx_request)
    }

    fn description(&self) -> &'static str {
        "Approve token"
    }
}
