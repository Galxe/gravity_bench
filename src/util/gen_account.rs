use std::{collections::HashMap, sync::Arc};

use alloy::{
    primitives::{keccak256, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

#[derive(Default)]
pub struct AccountGenerator {
    offset: u64,
    cache: HashMap<String, HashMap<Arc<Address>, Arc<PrivateKeySigner>>>,
}

impl AccountGenerator {
    pub fn gen_or_get_accounts(
        &mut self,
        key: Option<impl Into<String>>,
        size: usize,
    ) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
        let key = key.map(|k| k.into());
        if let Some(key) = &key {
            if let Some(accounts) = self.cache.get(key) {
                return Ok(accounts.clone());
            }
        }
        let accounts =
            gen_deterministic_accounts((self.offset..self.offset + size as u64).into_par_iter())?;
        self.offset += size as u64;
        if let Some(key) = &key {
            self.cache.entry(key.clone()).or_insert(accounts.clone());
        }
        Ok(accounts)
    }
}

/// Deterministically generate n Ethereum accounts from a slice of u64 seeds.
///
/// # Arguments
///
/// * `seeds` - A slice of u64 values used as seeds for generating private keys.
///
/// # Returns
///
/// * `Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>>` - Map of generated accounts
pub fn gen_deterministic_accounts(
    seeds: impl IntoParallelIterator<Item = u64>,
) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
    let accounts = seeds
        .into_par_iter()
        .map(|seed| {
            let private_key_bytes = keccak256(seed.to_le_bytes());

            let signer = Arc::new(
                PrivateKeySigner::from_slice(private_key_bytes.as_slice())
                    .context("Failed to create deterministic signer")
                    .unwrap(),
            );

            let address = Arc::new(signer.address());

            (address, signer)
        })
        .collect::<HashMap<_, _>>();

    Ok(accounts)
}
