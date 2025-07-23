use std::{collections::HashMap, sync::Arc};

use alloy::{primitives::Address, signers::local::PrivateKeySigner};

pub struct AccountManager {
    account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>,
    account_status: HashMap<Arc<Address>, u32>,
    ready_accounts: Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)>,
    all_account_addresses: Vec<Arc<Address>>,
}

impl AccountManager {
    pub fn new(account_signers: HashMap<Arc<Address>, Arc<PrivateKeySigner>>) -> Self {
        let mut account_status = HashMap::new();
        let mut ready_accounts = Vec::new();
        let all_account_addresses = account_signers.keys().cloned().collect();
        for (addr, signer) in account_signers.iter() {
            account_status.insert(addr.clone(), 0);
            ready_accounts.push((signer.clone(), addr.clone(), 0));
        }
        Self {
            account_signers,
            account_status,
            ready_accounts,
            all_account_addresses,
        }
    }

    pub fn is_full_ready(&self) -> bool {
        self.ready_accounts.len() == self.all_account_addresses.len()
    }

    pub fn ready_len(&self) -> usize {
        self.ready_accounts.len()
    }

    #[allow(unused)]
    pub fn all_account_addresses_len(&self) -> usize {
        self.all_account_addresses.len()
    }

    pub(crate) fn len(&self) -> usize {
        self.account_signers.len()
    }

    /// Returns a list of ready accounts with their current nonces.
    pub fn fetch_ready_accounts(
        &mut self,
        size: Option<usize>,
    ) -> Vec<(Arc<PrivateKeySigner>, Arc<Address>, u32)> {
        match size {
            Some(n) if n < self.ready_accounts.len() => {
                let remaining = self.ready_accounts.split_off(n);
                let result = std::mem::take(&mut self.ready_accounts);
                self.ready_accounts = remaining;
                result
            }
            _ => {
                std::mem::take(&mut self.ready_accounts)
            }
        }
    }

    pub(crate) fn retry_current_nonce(&mut self, account: Arc<Address>) {
        if let Some(_) = self.account_status.get_mut(&account) {
            self.ready_accounts.push((
                self.account_signers.get(&account).unwrap().clone(),
                account.clone(),
                self.account_status.get(&account).unwrap().clone(),
            ));
        }
    }

    pub(crate) fn unlock_correct_nonce(&mut self, account: Arc<Address>, nonce: u32) {
        if let Some(status) = self.account_status.get_mut(&account) {
            *status = nonce;
            self.ready_accounts.push((
                self.account_signers.get(&account).unwrap().clone(),
                account.clone(),
                *status,
            ));
        }
    }

    pub(crate) fn unlock_next_nonce(&mut self, account: Arc<Address>) {
        if let Some(status) = self.account_status.get_mut(&account) {
            *status += 1;

            self.ready_accounts.push((
                self.account_signers.get(&account).unwrap().clone(),
                account.clone(),
                self.account_status.get(&account).unwrap().clone(),
            ));
        }
    }

    pub(crate) fn resume_all_accounts(&mut self) {
        for (account, status) in self.account_status.iter_mut() {
            self.ready_accounts.push((
                self.account_signers.get(account).unwrap().clone(),
                account.clone(),
                *status,
            ));
        }
    }
}
