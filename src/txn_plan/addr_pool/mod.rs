use alloy::primitives::Address;

pub trait AddressPool: Send + Sync + 'static {
    fn select_address(&self, excluded: &Address) -> Address;

    fn lock_address(&self, address: Address);

    fn unlock_address(&self, address: Address);
}

mod random_address_pool;    

pub use random_address_pool::RandomAddressPool;