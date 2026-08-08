use crate::{
    eth::{BENCH_MAX_FEE_PER_GAS, BENCH_MAX_PRIORITY_FEE_PER_GAS},
    txn_plan::FromTxnConstructor,
    util::gen_account::{AccountId, AccountManager},
};
use alloy::{
    eips::eip7702::Authorization,
    network::{TransactionBuilder, TransactionBuilder7702},
    primitives::{Address, U256},
    rpc::types::TransactionRequest,
    signers::SignerSync,
};
use anyhow::Context;

/// Self-sponsored EIP-7702 SetCode (type-4) constructor.
///
/// Each worker signs an authorization for itself (`authority == sender`)
/// with `auth.nonce = tx.nonce + 1` (Pectra: sender nonce is bumped before
/// authorization processing), then builds a type-4 tx that re-delegates to
/// a fixed target contract.
///
/// Chain effect on the sender: nonce advances by **2** (tx + auth).
pub struct Eip7702Constructor {
    pub chain_id: u64,
    pub delegate: Address,
}

impl Eip7702Constructor {
    pub fn new(chain_id: u64, delegate: Address) -> Self {
        Self { chain_id, delegate }
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

        // Call the delegate contract after set-code (empty calldata is fine
        // for a fallback-only target; the stress is on type-4 auth processing).
        let tx_request = TransactionRequest::default()
            .with_from(from_address)
            .with_to(self.delegate)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_max_priority_fee_per_gas(BENCH_MAX_PRIORITY_FEE_PER_GAS)
            .with_max_fee_per_gas(BENCH_MAX_FEE_PER_GAS)
            .with_gas_limit(EIP7702_SET_CODE_GAS_LIMIT)
            .with_authorization_list(vec![signed_auth]);

        Ok(tx_request)
    }

    fn description(&self) -> &'static str {
        "EIP-7702 SetCode (self-sponsored)"
    }

    fn nonce_increment(&self) -> u32 {
        // Tx nonce check bumps once, then the matching self-auth bumps again.
        2
    }
}

/// Gas limit for a type-4 SetCode tx with a single authorization and empty call.
pub const EIP7702_SET_CODE_GAS_LIMIT: u64 = 100_000;

/// Gas limit for deploying the minimal fallback-only delegate target.
pub const EIP7702_DELEGATE_DEPLOY_GAS_LIMIT: u64 = 300_000;

/// Minimal contract used as the EIP-7702 delegation target.
///
/// Creation bytecode deploys a one-byte runtime (`STOP` = `0x00`) so calls
/// to the delegate (or to an EOA that delegated to it) succeed with empty
/// return data. The previous solc empty-contract blob ended in `REVERT`
/// (`5f80fd`), which made every type-4 call land with `status=0x0`.
///
/// Layout: `PUSH1 1; PUSH1 12; PUSH1 0; CODECOPY; PUSH1 1; PUSH1 0; RETURN; STOP`
pub fn delegate_contract_bytecode() -> Vec<u8> {
    hex::decode("6001600c60003960016000f300").expect("invalid delegate bytecode hex")
}
