use alloy::primitives::Address;

use crate::util::gen_account::AccountId;

pub mod managed_address_pool;
#[allow(unused)]
pub mod weighted_address_pool;

#[async_trait::async_trait]
pub trait AddressPool: Send + Sync + 'static {
    /// Fetches a batch of ready sender accounts based on the internal sampling strategy.
    /// This operation should internally lock the accounts to prevent concurrent use.
    /// Returns (AccountId, nonce) pairs.
    fn fetch_senders(&self, count: usize) -> Vec<(AccountId, u32)>;

    /// Unlocks an account after a successful transaction and increments its nonce.
    fn unlock_next_nonce(&self, account: AccountId);

    /// Unlocks an account and updates its nonce to a specific value.
    fn unlock_correct_nonce(&self, account: AccountId, nonce: u32);

    /// Makes an account available again for retry, using the same nonce.
    fn retry_current_nonce(&self, account: AccountId);

    /// Resumes all accounts, making them available again.
    fn resume_all_accounts(&self);

    fn clean_ready_accounts(&self);

    /// Checks if all accounts in the pool are ready.
    fn is_full_ready(&self) -> bool;

    /// Returns the number of ready accounts.
    fn ready_len(&self) -> usize;

    /// Returns the total number of accounts in the pool.
    fn len(&self) -> usize;

    /// Selects a receiver address based on the internal sampling strategy.
    /// The excluded parameter is the address to exclude from selection.
    fn select_receiver(&self, excluded: &Address) -> Address;
}
