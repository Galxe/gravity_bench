use alloy::{
    consensus::{SignableTransaction, TxEnvelope},
    network::{TransactionBuilder, TxSignerSync},
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use tracing::debug;

/// Max fee per gas for bench transactions (1000 Gwei).
///
/// Must stay above Gravity's ~50 Gwei min base fee with headroom so that
/// `max_priority_fee_per_gas` (500 Gwei) still fully applies:
/// `effective_tip = min(tip, maxFee - baseFee)`. Tip must stay in the
/// hundreds of Gwei range — empirically 1 Gwei never promotes out of
/// gravity-reth's `queued` bucket.
///
/// Also kept low enough that worst-case EIP-7702 stress
/// (`EIP7702_SET_CODE_GAS_LIMIT` × this) stays under the public RPC
/// default `rpc.txfeecap` of 1 ETH: 350_000 × 1000 Gwei = 0.35 ETH.
/// (Previously 5000 Gwei made 7702 reserve 1.75 ETH and testnet rejected it.)
pub const BENCH_MAX_FEE_PER_GAS: u128 = 1_000_000_000_000;

/// Priority fee (tip) for bench transactions (500 Gwei).
///
/// See `BENCH_MAX_FEE_PER_GAS` — 1 Gwei was below the gravity-reth
/// txpool promotion threshold under load, so transactions never landed.
pub const BENCH_MAX_PRIORITY_FEE_PER_GAS: u128 = 500_000_000_000;

/// TxnBuilder - Build and sign transactions
pub struct TxnBuilder;

impl TxnBuilder {
    /// Build and sign transaction
    pub fn build_and_sign_transaction(
        tx_request: TransactionRequest,
        signer: &PrivateKeySigner,
    ) -> Result<TxEnvelope> {
        debug!("Building and signing transaction with request: {:?}", tx_request);
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
            .with_max_priority_fee_per_gas(BENCH_MAX_PRIORITY_FEE_PER_GAS)
            .with_max_fee_per_gas(BENCH_MAX_FEE_PER_GAS)
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
            .with_max_priority_fee_per_gas(BENCH_MAX_PRIORITY_FEE_PER_GAS)
            .with_max_fee_per_gas(BENCH_MAX_FEE_PER_GAS)
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
            .with_max_priority_fee_per_gas(BENCH_MAX_PRIORITY_FEE_PER_GAS)
            .with_max_fee_per_gas(BENCH_MAX_FEE_PER_GAS)
            .with_gas_limit(100_000);

        Ok(tx_request)
    }
}
