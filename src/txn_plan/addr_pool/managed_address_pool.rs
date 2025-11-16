use std::{collections::HashMap, sync::Arc};

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use parking_lot::Mutex;

use super::AddressPool;

struct Inner {
    account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>,
    account_status: HashMap<Arc<Address>, u32>,
    ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
    all_account_addresses: Vec<Arc<Address>>,
}

pub struct RandomAddressPool {
    inner: Mutex<Inner>,
}

impl RandomAddressPool {
    #[allow(unused)]
    pub fn new(
        account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>,
    ) -> Self {
        let mut account_status = HashMap::new();
        let mut ready_accounts = Vec::new();
        let all_account_addresses: Vec<Arc<Address>> = account_signers.keys().cloned().collect();
        for (addr, signer) in account_signers.iter() {
            // assume all address start from nonce, this is correct beacause a nonce too low error will trigger correct nonce
            let nonce = 0;
            account_status.insert(addr.clone(), nonce);
            ready_accounts.push((signer.clone(), addr.clone(), nonce));
        }

        let inner = Inner {
            account_signers,
            account_status,
            ready_accounts,
            all_account_addresses,
        };

        Self {
            inner: Mutex::new(inner),
        }
    }
}

impl AddressPool for RandomAddressPool {
    fn fetch_senders(&self, count: usize) -> Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)> {
        let mut inner = self.inner.lock();
        if count < inner.ready_accounts.len() {
            let remaining = inner.ready_accounts.split_off(count);
            let result = std::mem::take(&mut inner.ready_accounts);
            inner.ready_accounts = remaining;
            result
        } else {
            std::mem::take(&mut inner.ready_accounts)
        }
    }

    fn unlock_next_nonce(&self, account: Arc<Address>) {
        let mut inner = self.inner.lock();
        if let Some(status) = inner.account_status.get_mut(&account) {
            *status += 1;
            let signer = inner.account_signers.get(&account).unwrap().clone();
            let status = *inner.account_status.get(&account).unwrap();
            inner.ready_accounts.push((signer, account.clone(), status));
        }
    }

    fn unlock_correct_nonce(&self, account: Arc<Address>, nonce: u32) {
        let mut inner = self.inner.lock();
        if let Some(status) = inner.account_status.get_mut(&account) {
            *status = nonce;
            let status = *status;
            let signer = inner.account_signers.get(&account).unwrap().clone();
            inner.ready_accounts.push((signer, account.clone(), status));
        }
    }

    fn retry_current_nonce(&self, account: Arc<Address>) {
        let mut inner = self.inner.lock();
        if inner.account_status.get_mut(&account).is_some() {
            let signer = inner.account_signers.get(&account).unwrap().clone();
            let status = *inner.account_status.get(&account).unwrap();
            inner.ready_accounts.push((signer, account.clone(), status));
        }
    }

    fn resume_all_accounts(&self) {
        let mut inner = self.inner.lock();
        inner.ready_accounts = inner
            .all_account_addresses
            .iter()
            .map(|account| {
                let signer = inner.account_signers.get(account).unwrap().clone();
                let status = *inner.account_status.get(account).unwrap();
                (signer, account.clone(), status)
            })
            .collect();
    }

    fn is_full_ready(&self) -> bool {
        let inner = self.inner.lock();
        inner.ready_accounts.len() == inner.all_account_addresses.len()
    }

    fn ready_len(&self) -> usize {
        self.inner.lock().ready_accounts.len()
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
