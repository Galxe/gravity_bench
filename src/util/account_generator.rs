use crate::util::gen_account::gen_account_with_offset;
use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};

/// A stateful generator for creating deterministic, unique accounts.
/// It tracks an internal offset to ensure that subsequent calls do not produce duplicate accounts.
pub struct AccountGenerator {
    offset: u64,
}

impl AccountGenerator {
    /// Creates a new `AccountGenerator` with an initial offset of 0.
    pub fn new() -> Self {
        Self { offset: 0 }
    }

    /// Generates `n` unique, deterministic accounts.
    /// This method is mutable as it updates the internal offset after generation.
    pub fn gen(&mut self, n: usize) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
        let accounts = gen_account_with_offset(n, self.offset)?;
        self.offset += n as u64;
        Ok(accounts)
    }
}

impl Default for AccountGenerator {
    fn default() -> Self {
        Self::new()
    }
}
