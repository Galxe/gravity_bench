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
    pub fn with_capacity(faucet_accout: PrivateKeySigner) -> Self {
        Self {
            accout_signers: Vec::new(),
            accout_addresses: Vec::new(),
            faucet_accout,
            faucet_accout_id: AccountId(u32::MAX),
            init_nonces: Vec::new(),
        }
    }

    pub fn to_manager(mut self) -> AccountManager {
        self.accout_signers.shrink_to_fit();
        self.accout_addresses.shrink_to_fit();
        self.init_nonces.shrink_to_fit();
        
        // 打印内存使用统计
        self.print_memory_summary();
        
        Arc::new(self)
    }

    /// Calculate and print memory usage of AccountGenerator
    pub fn print_memory_summary(&self) {
        const GB: f64 = 1024.0 * 1024.0 * 1024.0;
        
        // Vec overhead (ptr, len, capacity)
        let vec_overhead = std::mem::size_of::<Vec<()>>() * 3;
        
        // accout_signers: Vec<PrivateKeySigner>
        // PrivateKeySigner contains 32-byte private key + signing_key structure
        let signer_size = std::mem::size_of::<PrivateKeySigner>();
        let signers_capacity = self.accout_signers.capacity() * signer_size;
        let signers_used = self.accout_signers.len() * signer_size;
        
        // accout_addresses: Vec<Address>
        // Address is 20 bytes
        let address_size = std::mem::size_of::<Address>();
        let addresses_capacity = self.accout_addresses.capacity() * address_size;
        let addresses_used = self.accout_addresses.len() * address_size;
        
        // init_nonces: Vec<Arc<AtomicU64>>
        // Arc pointer + reference count
        let arc_size = std::mem::size_of::<Arc<AtomicU64>>();
        let atomic_u64_size = std::mem::size_of::<AtomicU64>();
        let nonces_capacity = self.init_nonces.capacity() * arc_size;
        let nonces_used = self.init_nonces.len() * arc_size;
        let nonces_heap = self.init_nonces.len() * atomic_u64_size;
        
        // faucet_accout: PrivateKeySigner
        let faucet_size = signer_size;
        
        // faucet_accout_id: AccountId (u32)
        let faucet_id_size = std::mem::size_of::<AccountId>();
        
        // Total
        let total_capacity = signers_capacity + addresses_capacity + nonces_capacity + 
                           faucet_size + faucet_id_size + vec_overhead + nonces_heap;
        let total_used = signers_used + addresses_used + nonces_used + 
                       faucet_size + faucet_id_size + vec_overhead + nonces_heap;
        
        println!("╔══════════════════════════════════════════════════════════╗");
        println!("║        AccountGenerator Memory Usage Summary             ║");
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ Account Count: {:<43} ║", self.accout_signers.len());
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ accout_signers (Vec<PrivateKeySigner>):                  ║");
        println!("║   - Element size: {} bytes                               ║", signer_size);
        println!("║   - Used: {:<10.6} GB ({} elements)                  ║", 
                 signers_used as f64 / GB, self.accout_signers.len());
        println!("║   - Capacity: {:<10.6} GB ({} capacity)              ║", 
                 signers_capacity as f64 / GB, self.accout_signers.capacity());
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ accout_addresses (Vec<Address>):                         ║");
        println!("║   - Element size: {} bytes                                ║", address_size);
        println!("║   - Used: {:<10.6} GB ({} elements)                  ║", 
                 addresses_used as f64 / GB, self.accout_addresses.len());
        println!("║   - Capacity: {:<10.6} GB ({} capacity)              ║", 
                 addresses_capacity as f64 / GB, self.accout_addresses.capacity());
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ init_nonces (Vec<Arc<AtomicU64>>):                       ║");
        println!("║   - Arc pointer size: {} bytes                           ║", arc_size);
        println!("║   - Used: {:<10.6} GB ({} elements)                  ║", 
                 nonces_used as f64 / GB, self.init_nonces.len());
        println!("║   - Capacity: {:<10.6} GB ({} capacity)              ║", 
                 nonces_capacity as f64 / GB, self.init_nonces.capacity());
        println!("║   - Heap AtomicU64: {:<10.6} GB                          ║", nonces_heap as f64 / GB);
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ faucet_accout (PrivateKeySigner): {:<24} ║", 
                 format!("{:.9} GB", faucet_size as f64 / GB));
        println!("║ faucet_accout_id (AccountId/u32): {:<24} ║", 
                 format!("{:.9} GB", faucet_id_size as f64 / GB));
        println!("║ Vec struct overhead: {:<35} ║", 
                 format!("{:.9} GB", vec_overhead as f64 / GB));
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ Total Memory Used: {:<39} ║", 
                 format!("{:.6} GB", total_used as f64 / GB));
        println!("║ Total Memory Capacity: {:<35} ║", 
                 format!("{:.6} GB", total_capacity as f64 / GB));
        println!("║ Wasted Space: {:<44} ║", 
                 format!("{:.6} GB ({:.1}%)", 
                         (total_capacity - total_used) as f64 / GB,
                         (total_capacity - total_used) as f64 / total_capacity as f64 * 100.0));
        println!("╚══════════════════════════════════════════════════════════╝");
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
            self.accout_addresses.reserve_exact(res.len());
            self.accout_signers.reserve_exact(res.len());
            self.init_nonces.reserve(res.len());
            self.accout_addresses.extend(res.iter().map(|signer| signer.address()));
            self.accout_signers.extend(res);
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
