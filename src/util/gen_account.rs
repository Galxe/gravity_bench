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

pub struct AccountGenerator {
    accout_signers: Vec<PrivateKeySigner>,
    accout_addresses: Vec<Address>,
    faucet_accout: PrivateKeySigner,
    faucet_accout_id: AccountId,
    init_nonces: Vec<Arc<AtomicU64>>,
}

pub type AccountManager = Arc<AccountGenerator>;

impl AccountGenerator {
    pub fn with_capacity(_capacity: usize, faucet_accout: PrivateKeySigner) -> Self {
        Self {
            accout_signers: Vec::new(),
            accout_addresses: Vec::new(),
            faucet_accout,
            faucet_accout_id: AccountId(u32::MAX),
            init_nonces: Vec::new(),
        }
    }

    pub fn to_manager(self) -> AccountManager {
        Arc::new(self)
    }

    pub fn get_signer_by_id(&self, id: AccountId) -> &PrivateKeySigner {
        if id == self.faucet_accout_id {
            &self.faucet_accout
        } else {
            &self.accout_signers[id.0 as usize]
        }
    }

    pub fn faucet_accout_id(&self) -> AccountId {
        self.faucet_accout_id
    }

    pub fn get_address_by_id(&self, id: AccountId) -> Address {
        if id == self.faucet_accout_id {
            self.faucet_accout.address()
        } else {
            self.accout_signers[id.0 as usize].address()
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
        self.accout_addresses.iter().zip(self.init_nonces.iter().cloned())
    }

    pub fn account_ids_with_nonce(&self) -> impl Iterator<Item = (AccountId, Arc<AtomicU64>)> + '_ {
        (0..self.accout_signers.len()).map(|i| (AccountId(i as u32), self.init_nonces[i].clone()))
    }

    pub fn gen_account(&mut self, start_index: u64, size: u64) -> Result<Vec<AccountId>> {
        let begin_index = self.accout_signers.len() as u64;
        let end_index = start_index + size;
        if begin_index < end_index {
            let res = self.gen_deterministic_accounts(begin_index, end_index);
            self.accout_addresses.extend(res.iter().map(|signer| signer.address()));
            self.accout_signers.extend(res);
            self.init_nonces
                .extend((0..size).map(|_| Arc::new(AtomicU64::new(0))));
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
