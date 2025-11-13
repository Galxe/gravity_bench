use std::sync::Arc;

use crate::txn_plan::addr_pool::AddressPool;
use alloy::primitives::Address;

pub struct RandomAddressPool {
    addresses: Arc<Vec<Arc<Address>>>,
}

impl RandomAddressPool {
    pub fn new(addresses: Arc<Vec<Arc<Address>>>) -> Self {
        Self { addresses }
    }
}

impl AddressPool for RandomAddressPool {
    fn select_address(&self, excluded: &Address) -> Address {
        // random select a receiver address, ensure not to self
        loop {
            let idx = rand::random::<usize>() % self.addresses.len();
            let to_address = self.addresses[idx].clone();
            if to_address.as_ref() != excluded {
                return to_address.as_ref().clone();
            }
        }
    }

    fn lock_address(&self, _address: Address) {
        // Not implemented for this pool type
    }

    fn unlock_address(&self, _address: Address) {
        // Not implemented for this pool type
    }
}
