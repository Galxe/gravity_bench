use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    u32,
};

use alloy::{
    primitives::{keccak256, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountId(u32);

const CACHE_SIZE: usize = 1024 * 1024;

pub struct AccountSignerCache {
    signers: Vec<PrivateKeySigner>,
    size: usize,
}

impl AccountSignerCache {
    pub(crate) fn new(size: usize) -> Self {
        Self {
            signers: Vec::with_capacity(size),
            size,
        }
    }

    pub(crate) fn save_signer(&mut self, signer: PrivateKeySigner, account_id: AccountId) {
        if account_id.0 as usize >= self.size {
            return;
        }
        if account_id.0 as usize == self.signers.len() {
            self.signers.push(signer);
        } else {
            self.signers[account_id.0 as usize] = signer;
        }
    }

    pub(crate) fn get_signer(&self, index: usize) -> PrivateKeySigner {
        if index >= self.signers.len() {
            return Self::compute_signer(AccountId(index as u32));
        }
        self.signers[index].clone()
    }

    fn compute_signer(id: AccountId) -> PrivateKeySigner {
        let private_key_bytes = keccak256((id.0 as u64).to_le_bytes());
        PrivateKeySigner::from_slice(private_key_bytes.as_slice())
            .context("Failed to create deterministic signer")
            .unwrap()
    }
}

pub struct AccountGenerator {
    accout_signers: AccountSignerCache,
    accout_addresses: Vec<Address>,
    faucet_accout: PrivateKeySigner,
    faucet_accout_id: AccountId,
    init_nonces: Vec<Arc<AtomicU64>>,
}

pub type AccountManager = Arc<AccountGenerator>;

impl AccountGenerator {
    pub fn with_capacity(faucet_accout: PrivateKeySigner) -> Self {
        Self {
            accout_signers: AccountSignerCache::new(CACHE_SIZE),
            accout_addresses: Vec::new(),
            faucet_accout,
            faucet_accout_id: AccountId(u32::MAX),
            init_nonces: Vec::new(),
        }
    }

    pub fn to_manager(mut self) -> AccountManager {
        self.accout_addresses.shrink_to_fit();
        self.init_nonces.shrink_to_fit();
        Arc::new(self)
    }

    pub fn get_signer_by_id(&self, id: AccountId) -> PrivateKeySigner {
        if id == self.faucet_accout_id {
            self.faucet_accout.clone()
        } else {
            self.accout_signers.get_signer(id.0 as usize)
        }
    }

    pub fn faucet_accout_id(&self) -> AccountId {
        self.faucet_accout_id
    }

    pub fn get_address_by_id(&self, id: AccountId) -> Address {
        if id == self.faucet_accout_id {
            self.faucet_accout.address()
        } else {
            self.accout_addresses[id.0 as usize]
        }
    }

    pub fn init_nonce_map(&self) -> HashMap<Address, u64> {
        let mut map = HashMap::new();
        for (account, nonce) in self.accouts_nonce_iter() {
            map.insert(account.clone(), nonce.load(Ordering::Relaxed));
        }
        map
    }

    pub fn accouts_nonce_iter(&self) -> impl Iterator<Item = (&Address, Arc<AtomicU64>)> {
        self.accout_addresses
            .iter()
            .zip(self.init_nonces.iter().cloned())
    }

    pub fn account_ids_with_nonce(&self) -> impl Iterator<Item = (AccountId, Arc<AtomicU64>)> + '_ {
        (0..self.accout_addresses.len()).map(|i| (AccountId(i as u32), self.init_nonces[i].clone()))
    }

    pub fn gen_account(&mut self, start_index: u64, size: u64) -> Result<Vec<AccountId>> {
        let begin_index = self.accout_addresses.len() as u64;
        let end_index = start_index + size;
        if begin_index < end_index {
            let res = self.gen_deterministic_accounts(begin_index, end_index);
            self.accout_addresses.reserve_exact(res.len());
            self.init_nonces.reserve(res.len());
            self.accout_addresses
                .extend(res.iter().map(|signer| signer.address()));
            for (i, signer) in res.iter().enumerate() {
                self.accout_signers
                    .save_signer(signer.clone(), AccountId(i as u32));
            }
            self.init_nonces
                .extend((0..size).map(|_| Arc::new(AtomicU64::new(0))));
        }
        let mut res = Vec::with_capacity(size as usize);
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
