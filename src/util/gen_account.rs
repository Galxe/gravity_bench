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
    address_to_id: HashMap<Address, AccountId>,
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
            address_to_id: HashMap::new(),
            faucet_accout,
            faucet_accout_id: AccountId(u32::MAX),
            init_nonces: Vec::new(),
        }
    }

    pub fn to_manager(mut self) -> AccountManager {
        self.accout_addresses.shrink_to_fit();
        self.init_nonces.shrink_to_fit();
        self.address_to_id.shrink_to_fit();
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
            self.address_to_id.reserve(res.len());
            for (i, signer) in res.iter().enumerate() {
                let addr = signer.address();
                let account_id = AccountId((begin_index + i as u64) as u32);
                self.accout_addresses.push(addr);
                self.address_to_id.insert(addr, account_id);
                self.accout_signers.save_signer(signer.clone(), account_id);
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

impl AccountGenerator {
    /// Find account ID by address using O(1) hashmap lookup
    pub fn find_account_id_by_address(&self, address: &Address) -> Option<AccountId> {
        // Check if it's the faucet account
        if *address == self.faucet_accout.address() {
            return Some(self.faucet_accout_id);
        }
        self.address_to_id.get(address).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::keccak256;

    #[test]
    fn test_compute_account_address() {
        // Test computing address for specific account IDs
        let test_ids = vec![0, 1, 100, 1000, 10000, 100000, 1000000];

        println!("\n=== Account ID to Address Mapping ===");
        for id in test_ids {
            let private_key_bytes = keccak256((id as u64).to_le_bytes());
            let signer = PrivateKeySigner::from_slice(private_key_bytes.as_slice()).unwrap();
            let address = signer.address();

            println!("ID {}: Address = {:?}", id, address);
            println!("       Private Key = 0x{}", hex::encode(private_key_bytes));
        }
    }

    #[test]
    fn test_find_account_by_address() {
        // Find account ID for a specific address
        let target_address = "0x3ece3a612e4e8849a3eaf093c61683b1370f3418";

        println!("\n=== Searching for address {} ===", target_address);

        // Search final recipients in batches
        println!("Searching final recipients (0-999999) in batches...");
        let batch_size = 10000;
        for batch_start in (0..1000000).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(1000000);

            for id in batch_start..batch_end {
                let private_key_bytes = keccak256((id as u64).to_le_bytes());
                let signer = PrivateKeySigner::from_slice(private_key_bytes.as_slice()).unwrap();
                let address = format!("{:?}", signer.address()).to_lowercase();

                if address == target_address.to_lowercase() {
                    println!("FOUND! Account ID = {}", id);
                    println!("This is a final recipient account");
                    println!("Private Key = 0x{}", hex::encode(private_key_bytes));

                    // Calculate parent node
                    let degree = 10;
                    let parent_index_in_level5 = id / degree;
                    let parent_id = 1011110 + parent_index_in_level5;

                    println!("\nParent node calculation:");
                    println!("  Parent is in Level 5");
                    println!("  Parent index in Level 5: {}", parent_index_in_level5);
                    println!("  Parent account ID: {}", parent_id);

                    let parent_pk = keccak256((parent_id as u64).to_le_bytes());
                    let parent_signer = PrivateKeySigner::from_slice(parent_pk.as_slice()).unwrap();
                    println!("  Parent address: {:?}", parent_signer.address());

                    return;
                }
            }

            if batch_start % 100000 == 0 {
                println!("  Checked up to ID {}...", batch_end);
            }
        }

        println!("Address not found in final recipients");
    }

    #[test]
    fn test_faucet_tree_structure() {
        // Print the faucet tree structure with actual addresses
        println!("\n=== Faucet Tree Structure ===");

        let degree = 10;
        let total_accounts = 1000000;

        // Faucet account (using the one from bench_config.toml)
        let faucet_pk = "5c173b12be434289682782ac6f7e7bf73a6fa5a20d507e318a4bdb039b1a5f6e";
        let faucet_bytes = hex::decode(faucet_pk).unwrap();
        let faucet_signer = PrivateKeySigner::from_slice(&faucet_bytes).unwrap();
        println!("Faucet: {:?}", faucet_signer.address());

        // Level 0 (first 10 intermediate accounts)
        println!("\nLevel 0 (indices 1000000-1000009):");
        for i in 0..3 {
            let id = 1000000 + i;
            let pk = keccak256((id as u64).to_le_bytes());
            let signer = PrivateKeySigner::from_slice(pk.as_slice()).unwrap();
            println!("  ID {}: {:?}", id, signer.address());
        }
        println!("  ...");

        // Level 1 (sample)
        println!("\nLevel 1 (indices 1000010-1000109, showing first 3):");
        for i in 0..3 {
            let id = 1000010 + i;
            let pk = keccak256((id as u64).to_le_bytes());
            let signer = PrivateKeySigner::from_slice(pk.as_slice()).unwrap();
            println!("  ID {}: {:?}", id, signer.address());
        }
        println!("  ...");
    }
}
