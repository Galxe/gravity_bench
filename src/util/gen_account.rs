use std::{
    collections::HashMap,
    sync::{
        Arc, atomic::{AtomicU64, Ordering}
    }, u32,
};

use alloy::{
    primitives::{keccak256, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountId(u32);

pub struct AccountGenerator {
    accouts: Vec<PrivateKeySigner>,
    faucet_accout: PrivateKeySigner,
    faucet_accout_id: AccountId,
    accout_to_id: HashMap<Address, AccountId>,
    init_nonces: Vec<Arc<AtomicU64>>,
}

pub type AccountManager = Arc<AccountGenerator>;

impl AccountGenerator {
    pub fn with_capacity(capacity: usize, faucet_accout: PrivateKeySigner) -> Self {
        Self {
            accouts: Vec::with_capacity(capacity),
            faucet_accout,
            faucet_accout_id: AccountId(u32::MAX),
            accout_to_id: HashMap::with_capacity(capacity),
            init_nonces: Vec::with_capacity(capacity),
        }
    }

    pub fn to_manager(self) -> AccountManager {
        Arc::new(self)
    }

    pub fn get_signer_by_id(&self, id: AccountId) -> &PrivateKeySigner {
        if id == self.faucet_accout_id {
            &self.faucet_accout
        } else {
            &self.accouts[id.0 as usize]
        }
    }

    pub fn get_address_by_id(&self, id: AccountId) -> Address {
        if id == self.faucet_accout_id {
            self.faucet_accout.address()
        } else {
            self.accouts[id.0 as usize].address()
        }
    }

    pub fn get_id_by_address(&self, address: &Address) -> Option<AccountId> {
        if address == &self.faucet_accout.address() {
            Some(self.faucet_accout_id)
        } else {
            self.accout_to_id.get(address).copied()
        }
    }

    pub fn init_nonce_map(&self) -> HashMap<Address, u64> {
        let mut map = HashMap::new();
        for (account, nonce) in self.accouts_nonce_iter() {
            map.insert(account.address(), nonce.load(Ordering::Relaxed));
        }
        map
    }

    pub fn accouts_nonce_iter(&self) -> impl Iterator<Item = (&PrivateKeySigner, Arc<AtomicU64>)> {
        self.accouts.iter().zip(self.init_nonces.iter().cloned())
    }

    pub fn account_ids_with_nonce(&self) -> impl Iterator<Item = (AccountId, Arc<AtomicU64>)> + '_ {
        (0..self.accouts.len()).map(|i| (AccountId(i as u32), self.init_nonces[i].clone()))
    }

    pub fn gen_account(
        &mut self,
        start_index: u64,
        size: u64,
    ) -> Result<Vec<AccountId>> {
        let begin_index = self.accouts.len() as u64;
        let end_index = start_index + size;
        if begin_index < end_index {
            let res = self.gen_deterministic_accounts(begin_index, end_index);
            self.accouts.extend(res);
            self.init_nonces
                .extend((0..size).map(|_| Arc::new(AtomicU64::new(0))));
            for i in begin_index..end_index {
                self.accout_to_id
                    .insert(self.accouts[i as usize].address(), AccountId(i as u32));
            }
        }
        let mut res = Vec::new();
        for i in 0..size {
            res.push(AccountId((start_index + i) as u32));
        }
        Ok(res)
    }

    fn gen_deterministic_accounts(
        &self,
        start_index: u64,
        end_index: u64,
    ) -> Vec<PrivateKeySigner> {
        let accounts = (start_index..end_index)
            .into_par_iter()
            .map(|seed| {
                let private_key_bytes = keccak256(seed.to_le_bytes());

                let signer = PrivateKeySigner::from_slice(private_key_bytes.as_slice())
                    .context("Failed to create deterministic signer")
                    .unwrap();
                signer
            })
            .collect::<Vec<_>>();

        accounts
    }
}
