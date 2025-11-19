use alloy::{
    consensus::{SignableTransaction, TxEnvelope},
    network::{TransactionBuilder, TxSignerSync},
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use tracing::debug;

/// TxnBuilder - Build and sign transactions
pub struct TxnBuilder;

impl TxnBuilder {
    /// Build and sign transaction
    pub fn build_and_sign_transaction(
        tx_request: TransactionRequest,
        signer: &PrivateKeySigner,
    ) -> Result<TxEnvelope> {
        debug!(
            "Building and signing transaction with request: {:?}",
            tx_request
        );
        debug!("Signer address: {:?}", signer.address());
        let mut unsigned_tx = tx_request.build_unsigned().unwrap();
        let sig = signer.sign_transaction_sync(&mut unsigned_tx)?;
        let tx_envelope = unsigned_tx.into_signed(sig);

        debug!("Transaction built and signed successfully");
        Ok(tx_envelope.into())
    }

    /// Build Uniswap V2 ETH for Token transaction request
    pub fn build_swap_exact_eth_for_tokens_request(
        router_address: Address,
        amount_out_min: U256,
        path: Vec<Address>,
        to: Address,
        deadline: U256,
        eth_amount: U256,
        nonce: u64,
        chain_id: u64,
    ) -> Result<TransactionRequest> {
        use crate::config::IUniswapV2Router;
        use alloy::sol_types::SolCall;

        let swap_call = IUniswapV2Router::swapExactETHForTokensCall {
            amountOutMin: amount_out_min,
            path,
            to,
            deadline,
        };

        let call_data = swap_call.abi_encode();
        let call_data = Bytes::from(call_data);

        let tx_request = TransactionRequest::default()
            .with_to(router_address)
            .with_input(call_data)
            .with_value(eth_amount)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(10_000_000_000) // 1 gwei
            .with_max_fee_per_gas(10_000_000_000) // 10 gwei
            .with_gas_limit(300_000);

        Ok(tx_request)
    }

    #[allow(unused)]
    pub fn eth_transfer_request(
        from: Address,
        to: Address,
        amount: U256,
        nonce: u64,
        chain_id: u64,
    ) -> Result<TransactionRequest> {
        let tx_request = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_value(amount)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(100_000_000)
            .with_max_fee_per_gas(100_000_000)
            .with_gas_limit(100_000);

        Ok(tx_request)
    }

    #[allow(unused)]
    pub fn self_eth_transfer_request(
        to: Address,
        amount: U256,
        nonce: u64,
        chain_id: u64,
    ) -> Result<TransactionRequest> {
        let tx_request = TransactionRequest::default()
            .with_to(to)
            .with_value(amount)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(100_000_000)
            .with_max_fee_per_gas(100_000_000)
            .with_gas_limit(100_000);

        Ok(tx_request)
    }
}
