use std::{collections::HashMap, sync::Arc};

use alloy::primitives::Address;
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use tokio::sync::RwLock;

use super::AddressPool;
use crate::util::gen_account::{AccountGenerator, AccountId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum AccountCategory {
    Hot,
    Normal,
    LongTail,
}

struct Inner {
    // Static data
    account_categories: HashMap<AccountId, AccountCategory>,
    all_account_ids: Vec<AccountId>,
    account_id_to_address: HashMap<AccountId, Address>,
    address_to_account_id: HashMap<Address, AccountId>,

    // Dynamic data
    account_status: HashMap<AccountId, u32>,
    hot_ready_accounts: Vec<(AccountId, u32)>,
    normal_ready_accounts: Vec<(AccountId, u32)>,
    long_tail_ready_accounts: Vec<(AccountId, u32)>,
}

pub struct WeightedAddressPool {
    inner: Mutex<Inner>,
}

impl WeightedAddressPool {
    pub fn new(account_ids: Vec<AccountId>, account_generator: Arc<RwLock<AccountGenerator>>) -> Self {
        let mut all_account_ids = account_ids;
        // Shuffle for random distribution
        all_account_ids.shuffle(&mut rand::thread_rng());

        let total_accounts = all_account_ids.len();
        let hot_count = (total_accounts as f64 * 0.2).round() as usize;
        let normal_count = (total_accounts as f64 * 0.1).round() as usize;

        let mut account_categories = HashMap::new();
        let mut hot_accounts = Vec::with_capacity(hot_count);
        let mut normal_accounts = Vec::with_capacity(normal_count);
        let mut long_tail_accounts = Vec::with_capacity(total_accounts - hot_count - normal_count);
        let mut account_id_to_address = HashMap::new();
        let mut address_to_account_id = HashMap::new();

        // Build address mapping synchronously by blocking on the async read
        let gen = tokio::runtime::Handle::current().block_on(account_generator.read());
        
        for (i, &account_id) in all_account_ids.iter().enumerate() {
            let address = gen.get_address_by_id(account_id);
            account_id_to_address.insert(account_id, address);
            address_to_account_id.insert(address, account_id);
            
            if i < hot_count {
                account_categories.insert(account_id, AccountCategory::Hot);
                hot_accounts.push(account_id);
            } else if i < hot_count + normal_count {
                account_categories.insert(account_id, AccountCategory::Normal);
                normal_accounts.push(account_id);
            } else {
                account_categories.insert(account_id, AccountCategory::LongTail);
                long_tail_accounts.push(account_id);
            }
        }
        drop(gen);

        let mut account_status = HashMap::new();
        let mut hot_ready_accounts = Vec::new();
        let mut normal_ready_accounts = Vec::new();
        let mut long_tail_ready_accounts = Vec::new();

        for &account_id in &all_account_ids {
            let nonce = 0;
            account_status.insert(account_id, nonce);
            let ready_tuple = (account_id, nonce);
            match account_categories.get(&account_id).unwrap() {
                AccountCategory::Hot => hot_ready_accounts.push(ready_tuple),
                AccountCategory::Normal => normal_ready_accounts.push(ready_tuple),
                AccountCategory::LongTail => long_tail_ready_accounts.push(ready_tuple),
            }
        }

        let inner = Inner {
            account_categories,
            all_account_ids,
            account_id_to_address,
            address_to_account_id,
            account_status,
            hot_ready_accounts,
            normal_ready_accounts,
            long_tail_ready_accounts,
        };

        Self {
            inner: Mutex::new(inner),
        }
    }

    fn unlock_account(&self, account: AccountId, nonce: Option<u32>) {
        let mut inner = self.inner.lock();
        if let Some(current_nonce) = inner.account_status.get_mut(&account) {
            let new_nonce = match nonce {
                Some(n) => n,
                None => *current_nonce + 1,
            };
            *current_nonce = new_nonce;

            let ready_tuple = (account, new_nonce);

            match inner.account_categories.get(&account).unwrap() {
                AccountCategory::Hot => inner.hot_ready_accounts.push(ready_tuple),
                AccountCategory::Normal => inner.normal_ready_accounts.push(ready_tuple),
                AccountCategory::LongTail => inner.long_tail_ready_accounts.push(ready_tuple),
            }
        }
    }
}

impl AddressPool for WeightedAddressPool {
    fn fetch_senders(&self, count: usize) -> Vec<(AccountId, u32)> {
        let mut inner = self.inner.lock();
        let mut result = Vec::with_capacity(count);

        let hot_target = (count as f64 * 0.7).round() as usize;
        let normal_target = (count as f64 * 0.2).round() as usize;
        let long_tail_target = count - hot_target - normal_target; // Ensure total is count

        // Drain from each category up to the target
        let hot_taken = std::cmp::min(hot_target, inner.hot_ready_accounts.len());
        result.extend(inner.hot_ready_accounts.drain(..hot_taken));

        let normal_taken = std::cmp::min(normal_target, inner.normal_ready_accounts.len());
        result.extend(inner.normal_ready_accounts.drain(..normal_taken));

        let long_tail_taken = std::cmp::min(long_tail_target, inner.long_tail_ready_accounts.len());
        result.extend(inner.long_tail_ready_accounts.drain(..long_tail_taken));

        // If we still need more, fill from any available pool
        let remaining = count - result.len();
        if remaining > 0 {
            let hot_fill = std::cmp::min(remaining, inner.hot_ready_accounts.len());
            result.extend(inner.hot_ready_accounts.drain(..hot_fill));
            let remaining = remaining - hot_fill;

            if remaining > 0 {
                let normal_fill = std::cmp::min(remaining, inner.normal_ready_accounts.len());
                result.extend(inner.normal_ready_accounts.drain(..normal_fill));
                let remaining = remaining - normal_fill;

                if remaining > 0 {
                    let long_tail_fill =
                        std::cmp::min(remaining, inner.long_tail_ready_accounts.len());
                    result.extend(inner.long_tail_ready_accounts.drain(..long_tail_fill));
                }
            }
        }

        result
    }

    fn clean_ready_accounts(&self) {
        let mut inner = self.inner.lock();
        inner.hot_ready_accounts.clear();
        inner.normal_ready_accounts.clear();
        inner.long_tail_ready_accounts.clear();
    }

    fn unlock_next_nonce(&self, account: AccountId) {
        self.unlock_account(account, None);
    }

    fn unlock_correct_nonce(&self, account: AccountId, nonce: u32) {
        self.unlock_account(account, Some(nonce));
    }

    fn retry_current_nonce(&self, account: AccountId) {
        let mut inner = self.inner.lock();

        let maybe_data = if let Some(nonce) = inner.account_status.get(&account) {
            let category = *inner.account_categories.get(&account).unwrap();
            Some((*nonce, category))
        } else {
            None
        };

        if let Some((nonce, category)) = maybe_data {
            let ready_tuple = (account, nonce);
            match category {
                AccountCategory::Hot => inner.hot_ready_accounts.push(ready_tuple),
                AccountCategory::Normal => inner.normal_ready_accounts.push(ready_tuple),
                AccountCategory::LongTail => inner.long_tail_ready_accounts.push(ready_tuple),
            }
        }
    }

    fn resume_all_accounts(&self) {
        let mut inner = self.inner.lock();
        inner.hot_ready_accounts.clear();
        inner.normal_ready_accounts.clear();
        inner.long_tail_ready_accounts.clear();
        let mut hot_ready_accounts = Vec::new();
        let mut normal_ready_accounts = Vec::new();
        let mut long_tail_ready_accounts = Vec::new();

        for &account_id in &inner.all_account_ids {
            let maybe_data = if let Some(nonce) = inner.account_status.get(&account_id) {
                let category = *inner.account_categories.get(&account_id).unwrap();
                Some((*nonce, category))
            } else {
                None
            };

            if let Some((nonce, category)) = maybe_data {
                let ready_tuple = (account_id, nonce);
                match category {
                    AccountCategory::Hot => hot_ready_accounts.push(ready_tuple),
                    AccountCategory::Normal => normal_ready_accounts.push(ready_tuple),
                    AccountCategory::LongTail => long_tail_ready_accounts.push(ready_tuple),
                }
            }
        }

        inner.hot_ready_accounts = hot_ready_accounts;
        inner.normal_ready_accounts = normal_ready_accounts;
        inner.long_tail_ready_accounts = long_tail_ready_accounts;
    }

    fn is_full_ready(&self) -> bool {
        let inner = self.inner.lock();
        let ready_count = inner.hot_ready_accounts.len()
            + inner.normal_ready_accounts.len()
            + inner.long_tail_ready_accounts.len();
        ready_count == inner.all_account_ids.len()
    }

    fn ready_len(&self) -> usize {
        let inner = self.inner.lock();
        inner.hot_ready_accounts.len()
            + inner.normal_ready_accounts.len()
            + inner.long_tail_ready_accounts.len()
    }

    fn len(&self) -> usize {
        self.inner.lock().all_account_ids.len()
    }

    fn select_receiver(&self, excluded: &Address) -> Address {
        let inner = self.inner.lock();
        let excluded_id = inner.address_to_account_id.get(excluded);
        loop {
            let idx = rand::random::<usize>() % inner.all_account_ids.len();
            let account_id = inner.all_account_ids[idx];
            if Some(&account_id) != excluded_id {
                return *inner.account_id_to_address.get(&account_id).unwrap();
            }
        }
    }
}
