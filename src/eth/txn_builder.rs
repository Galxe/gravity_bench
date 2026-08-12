use alloy::{
    consensus::{SignableTransaction, TxEnvelope},
    network::{TransactionBuilder, TxSignerSync},
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use std::sync::OnceLock;
use tracing::debug;

/// Default max fee per gas (1000 Gwei) when `[fee]` is omitted from toml.
///
/// Historical notes: must stay above Gravity's ~50 Gwei min base fee with
/// headroom so tip fully applies (`effective_tip = min(tip, maxFee - baseFee)`).
/// Also keep `gasLimit × maxFee` under public RPC `rpc.txfeecap` (1 ETH):
/// 350_000 × 1000 Gwei = 0.35 ETH for EIP-7702.
pub const BENCH_MAX_FEE_PER_GAS: u128 = 1_000_000_000_000;

/// Default priority fee / tip (500 Gwei) when `[fee]` is omitted from toml.
///
/// Empirically raised in #48 for high-TPS promote under load; idle testnet
/// often works with far lower tips — override via config after probing.
pub const BENCH_MAX_PRIORITY_FEE_PER_GAS: u128 = 500_000_000_000;

const GWEI: u128 = 1_000_000_000;

/// Runtime EIP-1559 fee caps for all bench-built transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GasFees {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

impl Default for GasFees {
    fn default() -> Self {
        Self {
            max_fee_per_gas: BENCH_MAX_FEE_PER_GAS,
            max_priority_fee_per_gas: BENCH_MAX_PRIORITY_FEE_PER_GAS,
        }
    }
}

impl GasFees {
    /// Build from Gwei units (toml-friendly).
    pub fn from_gwei(max_fee_gwei: u64, tip_gwei: u64) -> Self {
        Self {
            max_fee_per_gas: (max_fee_gwei as u128) * GWEI,
            max_priority_fee_per_gas: (tip_gwei as u128) * GWEI,
        }
    }

    pub fn max_fee_gwei(&self) -> u64 {
        (self.max_fee_per_gas / GWEI) as u64
    }

    pub fn tip_gwei(&self) -> u64 {
        (self.max_priority_fee_per_gas / GWEI) as u64
    }

    /// Mempool reserve / faucet gas budget slice: `gas_limit × maxFee` (wei).
    pub fn reserve_wei(&self, gas_limit: u64) -> u128 {
        self.max_fee_per_gas.checked_mul(gas_limit as u128).expect("gas_limit × max_fee overflow")
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.max_fee_per_gas == 0 {
            return Err("max_fee_per_gas must be > 0".into());
        }
        if self.max_priority_fee_per_gas > self.max_fee_per_gas {
            return Err(format!(
                "max_priority_fee_per_gas ({}) must be <= max_fee_per_gas ({})",
                self.max_priority_fee_per_gas, self.max_fee_per_gas
            ));
        }
        Ok(())
    }
}

static RUNTIME_GAS_FEES: OnceLock<GasFees> = OnceLock::new();

/// Install process-wide fee caps from config (call once after loading toml).
/// Subsequent calls are ignored; returns whether this call won the install.
pub fn init_gas_fees(fees: GasFees) -> bool {
    RUNTIME_GAS_FEES.set(fees).is_ok()
}

/// Current fee caps: configured runtime value, or compile-time defaults.
pub fn gas_fees() -> GasFees {
    RUNTIME_GAS_FEES.get().copied().unwrap_or_default()
}

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

        let fees = gas_fees();
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
            .with_max_priority_fee_per_gas(fees.max_priority_fee_per_gas)
            .with_max_fee_per_gas(fees.max_fee_per_gas)
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
        let fees = gas_fees();
        let tx_request = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_value(amount)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(fees.max_priority_fee_per_gas)
            .with_max_fee_per_gas(fees.max_fee_per_gas)
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
        let fees = gas_fees();
        let tx_request = TransactionRequest::default()
            .with_to(to)
            .with_value(amount)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(fees.max_priority_fee_per_gas)
            .with_max_fee_per_gas(fees.max_fee_per_gas)
            .with_gas_limit(100_000);

        Ok(tx_request)
    }
}
