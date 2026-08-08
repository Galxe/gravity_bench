use crate::{
    eth::{BENCH_MAX_FEE_PER_GAS, BENCH_MAX_PRIORITY_FEE_PER_GAS},
    txn_plan::{addr_pool::AddressPool, FromTxnConstructor},
    util::gen_account::{AccountId, AccountManager},
};
use alloy::{
    eips::eip7702::Authorization,
    network::{TransactionBuilder, TransactionBuilder7702},
    primitives::{Address, Bytes, U256},
    rpc::types::TransactionRequest,
    signers::SignerSync,
    sol,
    sol_types::SolCall,
};
use anyhow::Context;
use std::sync::Arc;

sol! {
    /// EIP-7702 delegation target: batch ETH transfers in the EOA context.
    interface IBatchExecutor {
        function multiSend(address[] calldata recipients, uint256[] calldata amounts) external payable;
    }
}

/// Default number of recipients per multiSend type-4 tx.
pub const EIP7702_DEFAULT_BATCH_SIZE: usize = 4;

/// Default ETH amount sent to each recipient (1 gwei).
/// Funds circulate among workers, so the amount can stay tiny.
pub const EIP7702_DEFAULT_AMOUNT_PER_RECIPIENT: u64 = 1_000_000_000;

/// Self-sponsored EIP-7702 SetCode (type-4) + ETH multi-send constructor.
///
/// Each worker:
/// 1. Signs an authorization for itself (`authority == sender`) with
///    `auth.nonce = tx.nonce + 1` (Pectra: sender nonce is bumped before
///    authorization processing).
/// 2. Builds a type-4 tx **to itself** with calldata
///    `multiSend(recipients, amounts)` so the delegated code runs in the
///    EOA context and spends the EOA's ETH.
///
/// Chain effect on the sender: nonce advances by **2** (tx + auth).
pub struct Eip7702Constructor {
    pub chain_id: u64,
    pub delegate: Address,
    pub address_pool: Arc<dyn AddressPool>,
    pub batch_size: usize,
    pub amount_per_recipient: U256,
}

impl Eip7702Constructor {
    pub fn new(
        chain_id: u64,
        delegate: Address,
        address_pool: Arc<dyn AddressPool>,
        batch_size: usize,
        amount_per_recipient: U256,
    ) -> Self {
        Self { chain_id, delegate, address_pool, batch_size, amount_per_recipient }
    }

    pub fn with_defaults(
        chain_id: u64,
        delegate: Address,
        address_pool: Arc<dyn AddressPool>,
    ) -> Self {
        Self::new(
            chain_id,
            delegate,
            address_pool,
            EIP7702_DEFAULT_BATCH_SIZE,
            U256::from(EIP7702_DEFAULT_AMOUNT_PER_RECIPIENT),
        )
    }
}

impl FromTxnConstructor for Eip7702Constructor {
    fn build_for_sender(
        &self,
        from_account_id: AccountId,
        account_generator: AccountManager,
        nonce: u64,
    ) -> Result<TransactionRequest, anyhow::Error> {
        let from_address = account_generator.get_address_by_id(from_account_id);
        let signer = account_generator.get_signer_by_id(from_account_id);

        let batch_size = self.batch_size.max(1);
        let mut recipients = Vec::with_capacity(batch_size);
        let mut amounts = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            let to_id = self.address_pool.select_receiver(from_account_id);
            recipients.push(account_generator.get_address_by_id(to_id));
            amounts.push(self.amount_per_recipient);
        }

        let call_data = Bytes::from(
            IBatchExecutor::multiSendCall { recipients, amounts }.abi_encode(),
        );

        // Self-sponsored: auth nonce is sender's post-tx-check nonce (= N+1).
        let auth = Authorization {
            chain_id: U256::from(self.chain_id),
            address: self.delegate,
            nonce: nonce + 1,
        };
        let sig = signer
            .sign_hash_sync(&auth.signature_hash())
            .context("failed to sign EIP-7702 authorization")?;
        let signed_auth = auth.into_signed(sig);

        // Critical: `to` must be the EOA so delegated runtime runs in EOA
        // context and multiSend spends the EOA's ETH. Calling the template
        // address would execute with address(this) == template.
        let tx_request = TransactionRequest::default()
            .with_from(from_address)
            .with_to(from_address)
            .with_input(call_data)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_max_priority_fee_per_gas(BENCH_MAX_PRIORITY_FEE_PER_GAS)
            .with_max_fee_per_gas(BENCH_MAX_FEE_PER_GAS)
            .with_gas_limit(EIP7702_SET_CODE_GAS_LIMIT)
            .with_authorization_list(vec![signed_auth]);

        Ok(tx_request)
    }

    fn description(&self) -> &'static str {
        "EIP-7702 SetCode + ETH multiSend (self-sponsored)"
    }

    fn nonce_increment(&self) -> u32 {
        // Tx nonce check bumps once, then the matching self-auth bumps again.
        2
    }
}

/// Gas limit for type-4 SetCode + multiSend (default K=4 cold-ish CALLs).
/// 7702 intrinsic/auth overhead + loop of value CALLs.
pub const EIP7702_SET_CODE_GAS_LIMIT: u64 = 350_000;

/// Gas limit for deploying the BatchExecutor delegate target.
pub const EIP7702_DELEGATE_DEPLOY_GAS_LIMIT: u64 = 500_000;

/// Creation bytecode for `contracts/BatchExecutor.sol` (solc 0.8.21, --optimize).
///
/// Runtime exposes `multiSend(address[],uint256[])` (selector `0xbb4c9f0b`).
/// Designed as an EIP-7702 delegation target: after set-code, call the EOA
/// with multiSend so value transfers use the EOA balance.
pub fn delegate_contract_bytecode() -> Vec<u8> {
    // solc --bin --optimize --optimize-runs 200 contracts/BatchExecutor.sol
    const HEX: &str = concat!(
        "608060405234801561000f575f80fd5b506102788061001d5f395ff3fe",
        "60806040526004361061001d575f3560e01c8063bb4c9f0b14610021575b5f80fd5b",
        "61003461002f36600461019a565b610036565b005b828181146100715760405162",
        "461bcd60e51b81526020600482015260036024820152623632b760e91b60448201",
        "526064015b60405180910390fd5b5f5b8181101561014a575f8686838181106100",
        "8e5761008e610201565b90506020020160208101906100a39190610215565b6001",
        "600160a01b03168585848181106100be576100be610201565b9050602002013560",
        "40515f6040518083038185875af1925050503d805f811461010257604051915060",
        "1f19603f3d011682016040523d82523d5f602084013e610107565b606091505b50",
        "509050806101415760405162461bcd60e51b815260040161006890602080825260",
        "0490820152631cd95b9960e21b604082015260600190565b50600101610073565b",
        "505050505050565b5f8083601f840112610162575f80fd5b50813567ffffffffff",
        "ffffff811115610179575f80fd5b6020830191508360208260051b850101111561",
        "0193575f80fd5b9250929050565b5f805f80604085870312156101ad575f80fd5b",
        "843567ffffffffffffffff808211156101c4575f80fd5b6101d088838901610152",
        "565b909650945060208701359150808211156101e8575f80fd5b506101f5878288",
        "01610152565b95989497509550505050565b634e487b7160e01b5f526032600452",
        "60245ffd5b5f60208284031215610225575f80fd5b81356001600160a01b038116",
        "811461023b575f80fd5b939250505056fea2646970667358221220e3f5701ab729",
        "c86aa306fb09342cfaf6071d057dbca36322a1d571b2ba2c024464736f6c634300",
        "08150033",
    );
    hex::decode(HEX).expect("invalid BatchExecutor creation bytecode hex")
}
