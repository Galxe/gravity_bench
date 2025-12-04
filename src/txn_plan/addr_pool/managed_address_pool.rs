use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;
use tokio::sync::RwLock;

use super::AddressPool;
use crate::util::gen_account::{AccountGenerator, AccountId, AccountManager};

struct Inner {
    account_status: HashMap<AccountId, u32>,
    ready_accounts: Vec<(AccountId, u32)>,
    all_account_ids: Vec<AccountId>,
}

pub struct RandomAddressPool {
    inner: Mutex<Inner>,
    account_generator: AccountManager,
}

impl RandomAddressPool {
    #[allow(unused)]
    pub fn new(account_ids: Vec<AccountId>, account_generator: AccountManager) -> Self {
        let mut account_status = HashMap::new();
        let mut ready_accounts = Vec::new();
        
        for &account_id in account_ids.iter() {
            // assume all address start from nonce, this is correct beacause a nonce too low error will trigger correct nonce
            let nonce = 0;
            account_status.insert(account_id, nonce);
            ready_accounts.push((account_id, nonce));
        }

        let inner = Inner {
            account_status,
            ready_accounts,
            all_account_ids: account_ids,
        };

        Self {
            inner: Mutex::new(inner),
            account_generator,
        }
    }
}

impl AddressPool for RandomAddressPool {
    fn fetch_senders(&self, count: usize) -> Vec<(AccountId, u32)> {
        let mut inner = self.inner.lock();
        let len = inner.ready_accounts.len();
        if count < len {
            inner.ready_accounts.split_off(len - count)
        } else {
            std::mem::take(&mut inner.ready_accounts)
        }
    }

    fn clean_ready_accounts(&self) {
        let mut inner = self.inner.lock();
        inner.ready_accounts.clear();
    }

    fn unlock_next_nonce(&self, account: AccountId) {
        let mut inner = self.inner.lock();
        if let Some(status) = inner.account_status.get_mut(&account) {
            *status += 1;
            let status = *inner.account_status.get(&account).unwrap();
            inner.ready_accounts.push((account, status));
        }
    }

    fn unlock_correct_nonce(&self, account: AccountId, nonce: u32) {
        let mut inner = self.inner.lock();
        if let Some(status) = inner.account_status.get_mut(&account) {
            *status = nonce;
            let status = *status;
            inner.ready_accounts.push((account, status));
        }
    }

    fn retry_current_nonce(&self, account: AccountId) {
        let mut inner = self.inner.lock();
        if inner.account_status.get_mut(&account).is_some() {
            let status = *inner.account_status.get(&account).unwrap();
            inner.ready_accounts.push((account, status));
        }
    }

    fn resume_all_accounts(&self) {
        let mut inner = self.inner.lock();
        inner.ready_accounts = inner
            .all_account_ids
            .iter()
            .map(|&account_id| {
                let status = *inner.account_status.get(&account_id).unwrap();
                (account_id, status)
            })
            .collect();
    }

    fn is_full_ready(&self) -> bool {
        let inner = self.inner.lock();
        inner.ready_accounts.len() == inner.all_account_ids.len()
    }

    fn ready_len(&self) -> usize {
        self.inner.lock().ready_accounts.len()
    }

    fn len(&self) -> usize {
        self.inner.lock().all_account_ids.len()
    }

    fn select_receiver(&self, excluded: AccountId) -> AccountId {
        let inner = self.inner.lock();
        
        loop {
            let idx = rand::random::<usize>() % inner.all_account_ids.len();
            let account_id = inner.all_account_ids[idx];
            if account_id != excluded {
                return account_id;
            }
        }
    }
}
