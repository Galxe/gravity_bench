use alloy::primitives::Address;

pub trait AddressPool {
    fn select_address(&self, excluded: &Address) -> Address;

    fn lock_address(&self, address: Address);

    fn unlock_address(&self, address: Address);
}
