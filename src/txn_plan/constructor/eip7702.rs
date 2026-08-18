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

        let call_data =
            Bytes::from(IBatchExecutor::multiSendCall { recipients, amounts }.abi_encode());

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
/// `multiSend` requires `msg.sender == address(this)` (onlySelf).
pub fn delegate_contract_bytecode() -> Vec<u8> {
    // solc --bin --optimize --optimize-runs 200 contracts/BatchExecutor.sol
    // Includes receive/fallback so multiSend into already-delegated EOAs succeeds.
    // Includes onlySelf guard so third parties cannot drain delegated EOAs.
    const HEX: &str = concat!(
        "608060405234801561000f575f80fd5b506102b28061001d5f395ff3fe608060",
        "40526004361061001e575f3560e01c8063bb4c9f0b1461002757005b36610025",
        "57005b005b6100256100353660046101d4565b3330146100755760405162461b",
        "cd60e51b815260206004820152600960248201526837b7363c9039b2b63360b9",
        "1b60448201526064015b60405180910390fd5b828181146100ab576040516246",
        "1bcd60e51b81526020600482015260036024820152623632b760e91b60448201",
        "5260640161006c565b5f5b81811015610184575f8686838181106100c8576100",
        "c861023b565b90506020020160208101906100dd919061024f565b6001600160",
        "a01b03168585848181106100f8576100f861023b565b90506020020135604051",
        "5f6040518083038185875af1925050503d805f811461013c576040519150601f",
        "19603f3d011682016040523d82523d5f602084013e610141565b606091505b50",
        "5090508061017b5760405162461bcd60e51b815260040161006c906020808252",
        "600490820152631cd95b9960e21b604082015260600190565b506001016100ad",
        "565b505050505050565b5f8083601f84011261019c575f80fd5b50813567ffff",
        "ffffffffffff8111156101b3575f80fd5b6020830191508360208260051b8501",
        "0111156101cd575f80fd5b9250929050565b5f805f80604085870312156101e7",
        "575f80fd5b843567ffffffffffffffff808211156101fe575f80fd5b61020a88",
        "83890161018c565b90965094506020870135915080821115610222575f80fd5b",
        "5061022f8782880161018c565b95989497509550505050565b634e487b7160e0",
        "1b5f52603260045260245ffd5b5f6020828403121561025f575f80fd5b813560",
        "01600160a01b0381168114610275575f80fd5b939250505056fea26469706673",
        "58221220361e305cccaa6daee31e970d468e4f32aa66c7ac631d269ae432b086",
        "4e4be77564736f6c63430008150033",
    );
    hex::decode(HEX).expect("invalid BatchExecutor creation bytecode hex")
}
