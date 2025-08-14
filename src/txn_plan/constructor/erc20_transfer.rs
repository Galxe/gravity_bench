use crate::{config::IERC20, txn_plan::FromTxnConstructor};
use alloy::{
    network::TransactionBuilder,
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use std::sync::Arc;

/// ERC20 transfer constructor
/// Transfer ERC20 tokens from accounts in the account manager to other addresses in the address list
pub struct Erc20TransferConstructor {
    pub token_list: Vec<Address>,
    pub transfer_amount: U256,
    pub chain_id: u64,
    pub address_list: Arc<Vec<Arc<Address>>>,
}

impl Erc20TransferConstructor {
    pub fn new(
        token_list: Vec<Address>,
        transfer_amount: U256,
        chain_id: u64,
        address_list: Arc<Vec<Arc<Address>>>,
    ) -> Self {
        Self {
            token_list,
            transfer_amount,
            chain_id,
            address_list,
        }
    }
}

impl FromTxnConstructor for Erc20TransferConstructor {
    fn build_for_sender(
        &self,
        from_account: &Arc<Address>,
        from_signer: &Arc<PrivateKeySigner>,
        nonce: u64,
    ) -> Result<TransactionRequest, anyhow::Error> {
        // random select a receiver address, ensure not to self
        let idx = rand::random::<usize>() % self.address_list.len();
        let mut to_address = self.address_list[idx].clone();

        // ensure not to self
        loop {
            if from_account == &to_address {
                let idx = rand::random::<usize>() % self.address_list.len();
                to_address = self.address_list[idx].clone();
            } else {
                break;
            }
        }

        // build ERC20 transfer call
        let transfer_call = IERC20::transferCall {
            to: *to_address.as_ref(),
            amount: self.transfer_amount,
        };

        let call_data = transfer_call.abi_encode();
        let call_data = Bytes::from(call_data);
        let token_idx = rand::random::<usize>() % self.token_list.len();
        let token_address = self.token_list[token_idx];
        // create transaction request
        let tx_request = TransactionRequest::default()
            .with_from(from_signer.address())
            .with_to(token_address)
            .with_input(call_data)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_max_priority_fee_per_gas(10_000_000_000) // 0.1 gwei
            .with_max_fee_per_gas(10_000_000_000) // 0.1 gwei
            .with_gas_limit(50_000); // Standard gas for ERC20 transfer

        Ok(tx_request)
    }

    fn description(&self) -> &'static str {
        "ERC20 token transfer"
    }
}
