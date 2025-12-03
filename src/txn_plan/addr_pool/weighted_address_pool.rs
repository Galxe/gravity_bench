use std::{collections::HashMap, sync::Arc};

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use parking_lot::Mutex;
use rand::seq::SliceRandom;

use super::AddressPool;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum AccountCategory {
    Hot,
    Normal,
    LongTail,
}

struct Inner {
    // Static data
    account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>,
    account_categories: HashMap<Arc<Address>, AccountCategory>,
    all_account_addresses: Vec<Arc<Address>>,

    // Dynamic data
    account_status: HashMap<Arc<Address>, u32>,
    hot_ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
    normal_ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
    long_tail_ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
}

pub struct WeightedAddressPool {
    inner: Mutex<Inner>,
}

impl WeightedAddressPool {
    pub fn new(account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>) -> Self {
        let mut all_account_addresses: Vec<Arc<Address>> =
            account_signers.keys().cloned().collect();
        // Shuffle for random distribution
        all_account_addresses.shuffle(&mut rand::thread_rng());

        let total_accounts = all_account_addresses.len();
        let hot_count = (total_accounts as f64 * 0.2).round() as usize;
        let normal_count = (total_accounts as f64 * 0.1).round() as usize;

        let mut account_categories = HashMap::new();
        let mut hot_accounts = Vec::with_capacity(hot_count);
        let mut normal_accounts = Vec::with_capacity(normal_count);
        let mut long_tail_accounts = Vec::with_capacity(total_accounts - hot_count - normal_count);

        for (i, addr) in all_account_addresses.iter().enumerate() {
            if i < hot_count {
                account_categories.insert(addr.clone(), AccountCategory::Hot);
                hot_accounts.push(addr.clone());
            } else if i < hot_count + normal_count {
                account_categories.insert(addr.clone(), AccountCategory::Normal);
                normal_accounts.push(addr.clone());
            } else {
                account_categories.insert(addr.clone(), AccountCategory::LongTail);
                long_tail_accounts.push(addr.clone());
            }
        }

        let mut account_status = HashMap::new();
        let mut hot_ready_accounts = Vec::new();
        let mut normal_ready_accounts = Vec::new();
        let mut long_tail_ready_accounts = Vec::new();

        for (addr, signer) in &account_signers {
            let nonce = 0;
            account_status.insert(addr.clone(), nonce);
            let ready_tuple = (signer.clone(), addr.clone(), nonce);
            match account_categories.get(addr).unwrap() {
                AccountCategory::Hot => hot_ready_accounts.push(ready_tuple),
                AccountCategory::Normal => normal_ready_accounts.push(ready_tuple),
                AccountCategory::LongTail => long_tail_ready_accounts.push(ready_tuple),
            }
        }

        let inner = Inner {
            account_signers,
            account_categories,
            all_account_addresses,
            account_status,
            hot_ready_accounts,
            normal_ready_accounts,
            long_tail_ready_accounts,
        };

        Self {
            inner: Mutex::new(inner),
        }
    }

    fn unlock_account(&self, account: Arc<Address>, nonce: Option<u32>) {
        let mut inner = self.inner.lock();
        if let Some(current_nonce) = inner.account_status.get_mut(&account) {
            let new_nonce = match nonce {
                Some(n) => n,
                None => *current_nonce + 1,
            };
            *current_nonce = new_nonce;

            let signer = inner.account_signers.get(&account).unwrap().clone();
            let ready_tuple = (signer, account.clone(), new_nonce);

            match inner.account_categories.get(&account).unwrap() {
                AccountCategory::Hot => inner.hot_ready_accounts.push(ready_tuple),
                AccountCategory::Normal => inner.normal_ready_accounts.push(ready_tuple),
                AccountCategory::LongTail => inner.long_tail_ready_accounts.push(ready_tuple),
            }
        }
    }
}

impl AddressPool for WeightedAddressPool {
    fn fetch_senders(&self, count: usize) -> Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)> {
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
        self.inner.lock().hot_ready_accounts.clear();
        self.inner.lock().normal_ready_accounts.clear();
        self.inner.lock().long_tail_ready_accounts.clear();
    }

    fn unlock_next_nonce(&self, account: Arc<Address>) {
        self.unlock_account(account, None);
    }

    fn unlock_correct_nonce(&self, account: Arc<Address>, nonce: u32) {
        self.unlock_account(account, Some(nonce));
    }

    fn retry_current_nonce(&self, account: Arc<Address>) {
        let mut inner = self.inner.lock();

        let maybe_data = if let Some(nonce) = inner.account_status.get(&account) {
            let signer = inner.account_signers.get(&account).unwrap().clone();
            let category = *inner.account_categories.get(&account).unwrap();
            Some((*nonce, signer, category))
        } else {
            None
        };

        if let Some((nonce, signer, category)) = maybe_data {
            let ready_tuple = (signer, account.clone(), nonce);
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

        for account in &inner.all_account_addresses {
            let maybe_data = if let Some(nonce) = inner.account_status.get(account) {
                let signer = inner.account_signers.get(account).unwrap().clone();
                let category = *inner.account_categories.get(account).unwrap();
                Some((*nonce, signer, category))
            } else {
                None
            };

            if let Some((nonce, signer, category)) = maybe_data {
                let ready_tuple = (signer, account.clone(), nonce);
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
        ready_count == inner.all_account_addresses.len()
    }

    fn ready_len(&self) -> usize {
        let inner = self.inner.lock();
        inner.hot_ready_accounts.len()
            + inner.normal_ready_accounts.len()
            + inner.long_tail_ready_accounts.len()
    }

    fn len(&self) -> usize {
        self.inner.lock().account_signers.len()
    }

    fn select_receiver(&self, excluded: &Address) -> Address {
        let inner = self.inner.lock();
        loop {
            let idx = rand::random::<usize>() % inner.all_account_addresses.len();
            let to_address = inner.all_account_addresses[idx].clone();
            if to_address.as_ref() != excluded {
                return *to_address;
            }
        }
    }
}
