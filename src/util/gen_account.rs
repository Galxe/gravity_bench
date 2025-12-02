use std::{collections::HashMap, sync::Arc};

use alloy::{
    primitives::{keccak256, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

#[derive(Default)]
pub struct AccountGenerator {
    accouts: Vec<PrivateKeySigner>,
    init_nonces: Vec<u64>,
}

impl AccountGenerator {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            accouts: Vec::with_capacity(capacity),
            init_nonces: Vec::with_capacity(capacity),
        }
    }

    pub fn accouts_nonce_mut(&mut self) -> impl Iterator<Item = (&PrivateKeySigner, &mut u64)> {
        self.accouts.iter().zip(self.init_nonces.iter_mut())
    }

    pub fn init_nonce(&mut self, idx: usize, nonce: u64) {
        self.init_nonces[idx] = nonce;
    }

    pub fn gen_account(&mut self, start_index: u64, size: u64) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
        let start_index = start_index.max(self.accouts.len() as u64);
        let res = self.gen_deterministic_accounts(start_index, start_index + size);
        self.accouts.extend(res);
        self.init_nonces.extend(vec![0; size as usize]);
        let mut res = HashMap::new();
        for i in 0..size {
            let signer = self.accouts[(start_index + i) as usize].clone();
            res.insert(Arc::new(signer.address()), Arc::new(signer));
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
    
                let signer = 
                    PrivateKeySigner::from_slice(private_key_bytes.as_slice())
                        .context("Failed to create deterministic signer")
                        .unwrap();
                signer
            })
            .collect::<Vec<_>>();
    
        accounts
    }
}
