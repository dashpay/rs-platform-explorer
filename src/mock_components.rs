//! Base (mock) components generic enough to be `MockComponent`, but missing from
//! the standard library of Realm components.

mod command_pallet;

pub(crate) use command_pallet::{CommandPallet, CommandPalletKey, KeyType};
