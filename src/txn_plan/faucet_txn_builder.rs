use alloy::{
    network::TransactionBuilder,
    primitives::{Address, U256},
    rpc::types::TransactionRequest,
    sol,
    sol_types::SolCall,
};

// Define the ERC20 interface for transfer
sol! {
    interface IERC20 {
        function transfer(address to, uint256 amount) external returns (bool);
    }
}

/// A trait for building faucet transactions. This allows for different types of
/// faucet transactions, such as native currency transfers or ERC20 token transfers.
pub trait FaucetTxnBuilder: Send + Sync {
    /// Builds a transaction request for a faucet transfer.
    fn build_faucet_txn(
        &self,
        to: Address,
        value: U256,
        nonce: u64,
        chain_id: u64,
    ) -> TransactionRequest;
}

/// A `FaucetTxnBuilder` for native Ethereum (ETH) transfers.
pub struct EthFaucetTxnBuilder;

impl FaucetTxnBuilder for EthFaucetTxnBuilder {
    fn build_faucet_txn(
        &self,
        to: Address,
        value: U256,
        nonce: u64,
        chain_id: u64,
    ) -> TransactionRequest {
        TransactionRequest::default()
            .with_to(to)
            .with_value(value)
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(10_000_000_000)
            .with_max_fee_per_gas(10_000_000_000)
            .with_gas_limit(21_000) // Standard gas for ETH transfer
    }
}

/// A `FaucetTxnBuilder` for ERC20 token transfers.
pub struct Erc20FaucetTxnBuilder {
    token_contract: Address,
}

impl Erc20FaucetTxnBuilder {
    pub fn new(token_contract: Address) -> Self {
        Self { token_contract }
    }
}

impl FaucetTxnBuilder for Erc20FaucetTxnBuilder {
    fn build_faucet_txn(
        &self,
        to: Address,
        value: U256,
        nonce: u64,
        chain_id: u64,
    ) -> TransactionRequest {
        // Create the ABI-encoded calldata for the ERC20 transfer function
        let calldata = IERC20::transferCall { to, amount: value };

        TransactionRequest::default()
            .with_to(self.token_contract)
            .with_value(U256::ZERO) // Value is 0 for token transfers
            .with_input(calldata.abi_encode())
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_max_priority_fee_per_gas(10_000_000_000)
            .with_max_fee_per_gas(10_000_000_000)
            .with_gas_limit(66_000) // A reasonable default for ERC20 transfers
    }
}
