//! Screens and forms related to wallet management.

use dpp::dashcore::psbt::serialize::Serialize;
use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::{
    backend::{
        identities::IdentityTask, AppState, AppStateUpdate, BackendEvent, Task, Wallet, WalletTask,
    },
    ui::{
        form::{
            parsers::DefaultTextInputParser, FormController, FormStatus, Input, InputStatus,
            TextInput,
        },
        screen::{
            info_display::InfoDisplay, utils::impl_builder, widgets::info::Info, ScreenCommandKey,
            ScreenController, ScreenFeedback, ScreenToggleKey,
        },
    },
    Event,
};

const WALLET_LOADED_COMMANDS: [ScreenCommandKey; 2] = [
    ScreenCommandKey::new("b", "Refresh wallet utxos and balance"),
    ScreenCommandKey::new("c", "Copy Receive Address"),
];

const IDENTITY_LOADED_COMMANDS: [ScreenCommandKey; 3] = [
    ScreenCommandKey::new("r", "Identity refresh"),
    ScreenCommandKey::new("w", "Withdraw balance"),
    ScreenCommandKey::new("d", "Copy Identity ID"),
];

#[memoize::memoize]
fn join_commands(
    wallet_loaded: bool,
    identity_loaded: bool,
    identity_registration_in_progress: bool,
    identity_top_up_in_progress: bool,
) -> &'static [ScreenCommandKey] {
    let mut commands = vec![ScreenCommandKey::new("q", "Back to Main")];

    if wallet_loaded {
        commands.extend_from_slice(&WALLET_LOADED_COMMANDS);
    } else {
        commands.push(ScreenCommandKey::new("a", "Add wallet by private key"));
    }

    if identity_loaded {
        commands.extend_from_slice(&IDENTITY_LOADED_COMMANDS);
        if identity_top_up_in_progress {
            commands.push(ScreenCommandKey::new("t", "Continue identity top up"));
        } else {
            commands.push(ScreenCommandKey::new("t", "Identity top up"));
        }
    } else {
        if identity_registration_in_progress {
            commands.push(ScreenCommandKey::new("i", "Continue identity registration"));
        } else {
            commands.push(ScreenCommandKey::new("i", "Register identity"));
        }
    }

    commands.leak()
}

pub(crate) struct WalletScreenController {
    wallet_info: Info,
    identity_info: Info,
    wallet_loaded: bool,
    identity_loaded: bool,
    identity_registration_in_progress: bool,
    identity_top_up_in_progress: bool,
}

impl_builder!(WalletScreenController);

struct RegisterIdentityFormController {
    input: TextInput<DefaultTextInputParser<f64>>,
}

impl RegisterIdentityFormController {
    fn new() -> Self {
        RegisterIdentityFormController {
            input: TextInput::new("Quantity (in Dash)"),
        }
    }
}

impl FormController for RegisterIdentityFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(amount) => FormStatus::Done {
                task: Task::Identity(IdentityTask::RegisterIdentity(
                    (amount * 100000000.0) as u64,
                )),
                block: true,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Identity registration"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Funding amount"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

struct TopUpIdentityFormController {
    input: TextInput<DefaultTextInputParser<f64>>,
}

impl TopUpIdentityFormController {
    fn new() -> Self {
        TopUpIdentityFormController {
            input: TextInput::new("Quantity (in Dash)"),
        }
    }
}

impl FormController for TopUpIdentityFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(amount) => FormStatus::Done {
                task: Task::Identity(IdentityTask::TopUpIdentity((amount * 100000000.0) as u64)),
                block: true,
            },
            InputStatus::Redraw => FormStatus::Redraw,
            InputStatus::None => FormStatus::None,
            InputStatus::Exit => FormStatus::Exit,
        }
    }

    fn form_name(&self) -> &'static str {
        "Identity top up"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Top up amount"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

struct WithdrawFromIdentityFormController {
    input: TextInput<DefaultTextInputParser<f64>>,
}

impl WithdrawFromIdentityFormController {
    fn new() -> Self {
        WithdrawFromIdentityFormController {
            input: TextInput::new("Quantity (in Dash)"),
        }
    }
}

impl FormController for WithdrawFromIdentityFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(amount) => FormStatus::Done {
                task: Task::Identity(IdentityTask::WithdrawFromIdentity(
                    (amount * 100000000.0) as u64,
                )),
                block: true,
            },
            InputStatus::Redraw => FormStatus::Redraw,
            InputStatus::None => FormStatus::None,
            InputStatus::Exit => FormStatus::Exit,
        }
    }

    fn form_name(&self) -> &'static str {
        "Identity withdrawal"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Withdrawal amount"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

impl WalletScreenController {
    pub(crate) async fn new(app_state: &AppState) -> Self {
        let (
            wallet_info,
            identity_info,
            wallet_loaded,
            identity_loaded,
            identity_registration_in_progress,
            identity_top_up_in_progress,
        ) = if let Some(wallet) = app_state.loaded_wallet.lock().await.as_ref() {
            if let Some(identity) = app_state.loaded_identity.lock().await.as_ref() {
                let identity_top_up_in_progress = app_state
                    .identity_asset_lock_private_key_in_top_up
                    .lock()
                    .await
                    .is_some();
                (
                    Info::new_fixed(&display_wallet(wallet)),
                    Info::new_fixed(&identity.display_info(0)),
                    true,
                    true,
                    false,
                    identity_top_up_in_progress,
                )
            } else {
                let identity_registration_in_progress = app_state
                    .identity_asset_lock_private_key_in_creation
                    .lock()
                    .await
                    .is_some();
                (
                    Info::new_fixed(&display_wallet(wallet)),
                    Info::new_fixed(""),
                    true,
                    false,
                    identity_registration_in_progress,
                    false,
                )
            }
        } else {
            (
                Info::new_fixed("Wallet management commands\n\nNo wallet loaded yet"),
                Info::new_fixed(""),
                false,
                false,
                false,
                false,
            )
        };

        WalletScreenController {
            wallet_info,
            identity_info,
            wallet_loaded,
            identity_loaded,
            identity_registration_in_progress,
            identity_top_up_in_progress,
        }
    }
}

impl ScreenController for WalletScreenController {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(20), Constraint::Min(20)].as_ref())
            .split(area);
        self.wallet_info.view(frame, layout[0]);
        self.identity_info.view(frame, layout[1]);
    }

    fn name(&self) -> &'static str {
        "Wallet"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        join_commands(
            self.wallet_loaded,
            self.identity_loaded,
            self.identity_registration_in_progress,
            self.identity_top_up_in_progress,
        )
    }

    fn toggle_keys(&self) -> &[ScreenToggleKey] {
        &[]
    }

    fn on_event(&mut self, event: &Event) -> ScreenFeedback {
        match event {
            Event::Key(KeyEvent {
                code: Key::Char('q'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::PreviousScreen,

            Event::Key(KeyEvent {
                code: Key::Char('a'),
                modifiers: KeyModifiers::NONE,
            }) if !self.wallet_loaded => {
                ScreenFeedback::Form(Box::new(AddWalletPrivateKeyFormController::new()))
            }

            Event::Key(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::NONE,
            }) if self.wallet_loaded => ScreenFeedback::Task {
                task: Task::Wallet(WalletTask::Refresh),
                block: true,
            },

            Event::Key(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) if self.identity_loaded => ScreenFeedback::Task {
                task: Task::Identity(IdentityTask::Refresh),
                block: true,
            },

            Event::Key(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::NONE,
            }) if self.identity_loaded => {
                ScreenFeedback::Form(Box::new(WithdrawFromIdentityFormController::new()))
            }

            Event::Key(KeyEvent {
                code: Key::Char('i'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(RegisterIdentityFormController::new())),

            Event::Key(KeyEvent {
                code: Key::Char('t'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(TopUpIdentityFormController::new())),

            Event::Key(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::NONE,
            }) if self.wallet_loaded => ScreenFeedback::Task {
                task: Task::Wallet(WalletTask::CopyAddress),
                block: true,
            },

            Event::Key(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) if self.identity_loaded => ScreenFeedback::Task {
                task: Task::Identity(IdentityTask::CopyIdentityId),
                block: true,
            },

            Event::Backend(
                BackendEvent::AppStateUpdated(AppStateUpdate::LoadedWallet(wallet))
                | BackendEvent::TaskCompletedStateChange {
                    app_state_update: AppStateUpdate::LoadedWallet(wallet),
                    ..
                },
            ) => {
                self.wallet_info = Info::new_fixed(&display_wallet(&wallet));
                self.wallet_loaded = true;
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompletedStateChange {
                task: Task::Identity(IdentityTask::RegisterIdentity(_)),
                execution_result,
                app_state_update: AppStateUpdate::IdentityRegistrationProgressed,
            }) => {
                self.identity_info = Info::new_from_result(execution_result);
                self.identity_loaded = false;
                self.identity_registration_in_progress = true;
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompletedStateChange {
                execution_result,
                app_state_update: AppStateUpdate::LoadedIdentity(identity),
                ..
            }) => {
                self.identity_loaded = true;
                self.identity_registration_in_progress = false;
                if execution_result.is_ok() {
                    self.identity_info = Info::new_fixed(&identity.display_info(0));
                } else {
                    self.identity_info = Info::new_from_result(execution_result);
                }
                self.identity_info = Info::new_fixed(&identity.display_info(0));
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Wallet(_),
                execution_result: Err(e),
                ..
            }) => {
                self.wallet_info = Info::new_error(&e);
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Identity(_),
                execution_result: Err(e),
                ..
            }) => {
                self.identity_info = Info::new_error(&e);
                ScreenFeedback::Redraw
            }
            _ => ScreenFeedback::None,
        }
    }
}

struct AddWalletPrivateKeyFormController {
    input: TextInput<DefaultTextInputParser<String>>, /* TODO: provide parser to always have a
                                                       * typesafe valid output */
}

impl AddWalletPrivateKeyFormController {
    fn new() -> Self {
        AddWalletPrivateKeyFormController {
            input: TextInput::new("64 hex character or WIF private key"),
        }
    }
}

impl FormController for AddWalletPrivateKeyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(private_key) => FormStatus::Done {
                task: Task::Wallet(WalletTask::AddByPrivateKey(private_key)),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Add wallet with private key"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Private key"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

fn display_wallet(wallet: &Wallet) -> String {
    match wallet {
        Wallet::SingleKeyWallet(single_key_wallet) => {
            let description = format!(
                "Single Key Wallet\nPublic Key: {}\nAddress: {}\nBalance: {}",
                hex::encode(single_key_wallet.public_key.serialize()),
                single_key_wallet.address,
                single_key_wallet.balance_dash_formatted()
            );
            let utxo_count = single_key_wallet.utxos.len();
            format!("{}\nNumber of UTXOs: {}", description, utxo_count)
        }
    }
}
