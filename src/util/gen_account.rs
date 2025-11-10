use std::{collections::HashMap, sync::Arc};

use alloy::{
    primitives::{keccak256, Address},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

/// Deterministically generate n Ethereum accounts with a seed offset.
///
/// # Arguments
///
/// * `n` - Number of accounts to generate
/// * `seed_offset` - The starting seed for generation
///
/// # Returns
///
/// * `Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>>` - Map of generated accounts
pub(crate) fn gen_account_with_offset(
    n: usize,
    seed_offset: u64,
) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
    if n == 0 {
        return Ok(HashMap::new());
    }

    let accounts = (0..n as u64)
        .into_par_iter()
        .map(|i| {
            let seed = seed_offset + i;
            let private_key_bytes = keccak256(seed.to_le_bytes());

            let signer = Arc::new(
                PrivateKeySigner::from_slice(private_key_bytes.as_slice())
                    .with_context(|| {
                        format!("Failed to create deterministic signer for account with seed {}", seed)
                    })
                    .unwrap(),
            );

            let address = Arc::new(signer.address());

            (address, signer)
        })
        .collect::<HashMap<_, _>>();
    Ok(accounts)
}
