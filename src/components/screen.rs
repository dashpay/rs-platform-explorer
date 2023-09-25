//! Application screens module.

mod get_identity;
mod identity;
mod main;

pub(crate) use get_identity::GetIdentityScreen;
pub(crate) use get_identity::GetIdentityScreenCommands;
pub(crate) use identity::IdentityScreen;
pub(crate) use identity::IdentityScreenCommands;
pub(crate) use main::MainScreen;
pub(crate) use main::MainScreenCommands;
