//! Screens and forms related to strategies manipulation.

use std::collections::BTreeMap;

use dpp::{
    data_contract::accessors::v0::DataContractV0Getters, platform_value::string_encoding::Encoding,
};
use strategy_tests::{
    operations::{
        DataContractUpdateAction::{DataContractNewDocumentTypes, DataContractNewOptionalFields},
        DocumentAction, OperationType,
    },
    Strategy,
};
use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

use super::{
    identity_inserts::IdentityInsertsScreenController, operations::OperationsScreenController,
    run_strategy::RunStrategyFormController, run_strategy::RunStrategyScreenController,
    start_contracts::StartContractsScreenController,
    start_identities::StartIdentitiesScreenController,
};
use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent},
    ui::screen::{
        utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
        ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

use crate::{
    backend::{StrategyTask, Task},
    ui::form::{
        parsers::DefaultTextInputParser, FormController, FormStatus, Input, InputStatus, TextInput,
    },
};

const COMMAND_KEYS: [ScreenCommandKey; 7] = [
    ScreenCommandKey::new("q", "Back to Strategies"),
    ScreenCommandKey::new("r", "Run strategy"),
    ScreenCommandKey::new("l", "Clone this strategy"),
    ScreenCommandKey::new("c", "Start contracts"),
    ScreenCommandKey::new("i", "Identity inserts"),
    ScreenCommandKey::new("o", "Operations"),
    ScreenCommandKey::new("s", "Start identities"),
];

const COMMAND_KEYS_NO_SELECTION: [ScreenCommandKey; 1] =
    [ScreenCommandKey::new("q", "Back to Strategies")];

pub struct SelectedStrategyScreenController {
    info: Info,
    available_strategies: Vec<String>,
    selected_strategy: Option<String>,
}

impl_builder!(SelectedStrategyScreenController);

impl SelectedStrategyScreenController {
    pub(crate) async fn new(app_state: &AppState) -> Self {
        let available_strategies_lock = app_state.available_strategies.lock().await;
        let selected_strategy_lock = app_state.selected_strategy.lock().await;

        let info = if let Some(name) = selected_strategy_lock.as_ref() {
            let strategy = available_strategies_lock
                .get(name.as_str())
                .expect("inconsistent data");
            let contract_names_lock = app_state.available_strategies_contract_names.lock().await;

            Info::new_fixed(&display_strategy(
                &name,
                strategy,
                contract_names_lock
                    .get(name.as_str())
                    .expect("inconsistent data"),
            ))
        } else {
            Info::new_fixed("No strategy selected. Go back.")
        };

        SelectedStrategyScreenController {
            info,
            available_strategies: available_strategies_lock.keys().cloned().collect(),
            selected_strategy: None,
        }
    }
}

impl ScreenController for SelectedStrategyScreenController {
    fn name(&self) -> &'static str {
        "Strategy"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        if self.selected_strategy.is_some() {
            COMMAND_KEYS.as_ref()
        } else {
            COMMAND_KEYS_NO_SELECTION.as_ref()
        }
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
                code: Key::Char('c'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::NextScreen(StartContractsScreenController::builder()),
            Event::Key(KeyEvent {
                code: Key::Char('i'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::NextScreen(IdentityInsertsScreenController::builder()),
            Event::Key(KeyEvent {
                code: Key::Char('s'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::NextScreen(StartIdentitiesScreenController::builder()),
            Event::Key(KeyEvent {
                code: Key::Char('o'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::NextScreen(OperationsScreenController::builder()),
            Event::Key(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::FormThenNextScreen {
                form: Box::new(RunStrategyFormController::new(
                    self.selected_strategy.clone().unwrap(),
                )),
                screen: RunStrategyScreenController::builder(),
            },
            Event::Key(KeyEvent {
                code: Key::Char('l'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(CloneStrategyFormController::new())),
            Event::Backend(
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name,
                    strategy,
                    contract_names,
                ))
                | BackendEvent::TaskCompletedStateChange {
                    app_state_update:
                        AppStateUpdate::SelectedStrategy(strategy_name, strategy, contract_names),
                    ..
                },
            ) => {
                self.info = Info::new_fixed(&display_strategy(
                    &strategy_name,
                    &strategy,
                    &contract_names,
                ));
                self.selected_strategy = Some(strategy_name.clone());
                ScreenFeedback::Redraw
            }
            Event::Backend(
                BackendEvent::AppStateUpdated(AppStateUpdate::Strategies(strategies, ..))
                | BackendEvent::TaskCompletedStateChange {
                    app_state_update: AppStateUpdate::Strategies(strategies, ..),
                    ..
                },
            ) => {
                self.available_strategies = strategies.keys().cloned().collect();
                ScreenFeedback::Redraw
            }
            _ => ScreenFeedback::None,
        }
    }

    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.info.view(frame, area)
    }
}

fn display_strategy(
    strategy_name: &str,
    strategy: &Strategy,
    contract_updates: &[(String, Option<BTreeMap<u64, String>>)],
) -> String {
    let mut start_contracts_lines = String::new();
    // Only display the individual contract details in this screen if the number is less than 5
    if contract_updates.len() <= 5 {
        for (contract, updates) in contract_updates.iter() {
            start_contracts_lines.push_str(&format!(
                "{:indent$}Contract: {contract}\n",
                "",
                indent = 8
            ));
            for (block, update) in updates.iter().flatten() {
                let block = block + 1;
                let block_spacing = (block - 1) * 3;
                start_contracts_lines.push_str(&format!(
                    "{:indent$}On block {block_spacing} apply {update}\n",
                    "",
                    indent = 12
                ));
            }
        }
    }

    let times_per_block_display = if strategy
        .identity_inserts
        .frequency
        .times_per_block_range
        .end
        > strategy
            .identity_inserts
            .frequency
            .times_per_block_range
            .start
    {
        strategy
            .identity_inserts
            .frequency
            .times_per_block_range
            .end
            - 1
    } else {
        strategy
            .identity_inserts
            .frequency
            .times_per_block_range
            .end
    };

    let identity_inserts_line = format!(
        "{:indent$}Times per block: {}; chance per block: {}\n",
        "",
        times_per_block_display,
        strategy
            .identity_inserts
            .frequency
            .chance_per_block
            .unwrap_or(0.0),
        indent = 8,
    );

    let mut operations_lines = String::new();
    // Only display the individual operation details in this screen if the number is less than 5
    if strategy.operations.len() <= 5 {
        for op in strategy.operations.iter() {
            let op_name = match op.op_type.clone() {
                OperationType::Document(op) => {
                    let op_type = match op.action {
                        DocumentAction::DocumentActionInsertRandom(..) => {
                            "InsertRandom".to_string()
                        }
                        DocumentAction::DocumentActionDelete => "Delete".to_string(),
                        // DocumentAction::DocumentActionReplace => "Replace".to_string(),
                        _ => panic!("invalid document action selected"),
                    };
                    format!(
                        "Document({}): Contract: {}",
                        op_type,
                        op.contract.id().to_string(Encoding::Base58)
                    )
                }
                OperationType::IdentityTopUp => "IdentityTopUp".to_string(),
                OperationType::IdentityUpdate(op) => format!("IdentityUpdate({:?})", op),
                OperationType::IdentityWithdrawal => "IdentityWithdrawal".to_string(),
                OperationType::ContractCreate(..) => "ContractCreateRandom".to_string(),
                OperationType::ContractUpdate(op) => {
                    let op_type = match op.action {
                        DataContractNewDocumentTypes(_) => "NewDocTypesRandom".to_string(),
                        DataContractNewOptionalFields(..) => "NewFieldsRandom".to_string(),
                    };
                    format!(
                        "ContractUpdate({}): Contract: {}",
                        op_type,
                        op.contract.id().to_string(Encoding::Base58)
                    )
                }
                OperationType::IdentityTransfer => "IdentityTransfer".to_string(),
            };

            let times_per_block_display = if op.frequency.times_per_block_range.end
                > op.frequency.times_per_block_range.start
            {
                op.frequency.times_per_block_range.end - 1
            } else {
                op.frequency.times_per_block_range.end
            };

            if times_per_block_display == 0 {
                operations_lines.push_str(&format!(
                    "{:indent$}{}; Times per block: {}, chance per block: {}\n",
                    "",
                    op_name,
                    times_per_block_display,
                    op.frequency.chance_per_block.unwrap_or(0.0),
                    indent = 8
                ));
            } else {
                operations_lines.push_str(&format!(
                    "{:indent$}{}; Times per block: {}, chance per block: {}\n",
                    "",
                    op_name,
                    times_per_block_display,
                    op.frequency.chance_per_block.unwrap_or(0.0),
                    indent = 8
                ));
            }
        }
    }

    let start_contracts_len = strategy.start_contracts.len();
    let operations_len = strategy.operations.len();

    format!(
        r#"{strategy_name}:
    Start identities: {} (Keys: {}, Balance: {:.2} dash)
    
    Start contracts ({start_contracts_len}):
{start_contracts_lines}
    Identity inserts:
{identity_inserts_line}
    Operations ({operations_len}):
{operations_lines}"#,
        strategy.start_identities.number_of_identities,
        strategy.start_identities.keys_per_identity
            + strategy.start_identities.extra_keys.len() as u8,
        strategy.start_identities.starting_balances as f64 / 100_000_000.0,
    )
}

pub(crate) struct CloneStrategyFormController {
    input: TextInput<DefaultTextInputParser<String>>,
}

impl CloneStrategyFormController {
    pub(crate) fn new() -> Self {
        CloneStrategyFormController {
            input: TextInput::new("strategy name"),
        }
    }
}

impl FormController for CloneStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(strategy_name) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::CloneStrategy(strategy_name)),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Clone strategy"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Strategy name"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}
