use std::{collections::HashMap, sync::Arc};

use alloy::{
    primitives::{keccak256, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use rand::{thread_rng, Rng};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

/// Randomly generate n Ethereum accounts
///
/// # Arguments
///
/// * `n` - Number of accounts to generate
///
/// # Returns
///
/// * `Result<Vec<GeneratedAccount>>` - List of generated accounts
///
/// # Examples
///
/// ```
/// let accounts = gen_account(5).unwrap();
/// for account in accounts {
///     println!("Address: {}, Private Key: {}", account.address_string(), account.private_key_hex());
/// }
/// ```
pub fn gen_account(n: usize) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
    if n == 0 {
        return Ok(HashMap::new());
    }

    let accounts = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut rng = thread_rng();
            let mut private_key_bytes = [0u8; 32];
            rng.fill(&mut private_key_bytes);

            let signer = Arc::new(
                PrivateKeySigner::from_slice(&private_key_bytes)
                    .with_context(|| format!("Failed to create signer for account {}", i))
                    .unwrap(),
            );

            let address = Arc::new(signer.address());

            (address, signer)
        })
        .collect::<HashMap<_, _>>();
    Ok(accounts)
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
    seeds: &[u64],
) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
    if seeds.is_empty() {
        return Ok(HashMap::new());
    }

    let accounts = seeds
        .into_par_iter()
        .map(|&seed| {
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
