//! Edit operations screen.

use std::collections::BTreeMap;

use dpp::{
    data_contract::{
        accessors::v0::DataContractV0Getters, created_data_contract::CreatedDataContract,
        DataContract,
    },
    platform_value::string_encoding::Encoding,
    tests::json_document::json_document_to_contract,
    version::PlatformVersion,
};
use drive::drive::contract;
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
use walkdir::WalkDir;

use super::operations::StrategyAddOperationFormController;
use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent, StrategyTask, Task},
    ui::screen::{
        utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
        ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

const COMMAND_KEYS: [ScreenCommandKey; 3] = [
    ScreenCommandKey::new("q", "Back to Strategy"),
    ScreenCommandKey::new("a", "Add"),
    ScreenCommandKey::new("r", "Remove last"),
];

pub struct OperationsScreenController {
    info: Info,
    strategy_name: Option<String>,
    selected_strategy: Option<Strategy>,
    contracts_with_updates: Vec<(
        CreatedDataContract,
        Option<BTreeMap<u64, CreatedDataContract>>,
    )>,
    known_contracts: BTreeMap<String, DataContract>,
    supporting_contracts: BTreeMap<String, DataContract>,
    strategy_contract_names: BTreeMap<String, Vec<(String, Option<BTreeMap<u64, String>>)>>,
}

impl_builder!(OperationsScreenController);

impl OperationsScreenController {
    pub(crate) async fn new(app_state: &AppState) -> Self {
        let available_strategies_lock = app_state.available_strategies.lock().await;
        let selected_strategy_lock = app_state.selected_strategy.lock().await;
        let known_contracts_lock = app_state.known_contracts.lock().await;
        let supporting_contracts_lock = app_state.supporting_contracts.lock().await;
        let strategy_contract_names_lock =
            app_state.available_strategies_contract_names.lock().await;

        let (info_text, current_strategy, current_contracts_with_updates) =
            if let Some(selected_strategy_name) = &*selected_strategy_lock {
                if let Some(strategy) = available_strategies_lock.get(selected_strategy_name) {
                    // Construct the info_text and get the contracts_with_updates for the selected
                    // strategy
                    let info_text = format!("Selected Strategy: {}", selected_strategy_name);
                    let current_contracts_with_updates = strategy.contracts_with_updates.clone();
                    (
                        info_text,
                        Some(strategy.clone()),
                        current_contracts_with_updates,
                    )
                } else {
                    ("No selected strategy found".to_string(), None, vec![])
                }
            } else {
                ("No strategy selected".to_string(), None, vec![])
            };

        let info = Info::new_fixed(&info_text);

        Self {
            info,
            strategy_name: selected_strategy_lock.clone(),
            selected_strategy: current_strategy,
            contracts_with_updates: current_contracts_with_updates,
            known_contracts: known_contracts_lock.clone(),
            supporting_contracts: supporting_contracts_lock.clone(),
            strategy_contract_names: strategy_contract_names_lock.clone(),
        }
    }

    async fn update_supporting_contracts(&mut self) {
        let platform_version = PlatformVersion::latest();

        for entry in WalkDir::new("supporting_files/contract")
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        {
            let path = entry.path();
            let contract_name = path.file_stem().unwrap().to_str().unwrap().to_string();

            if !self.supporting_contracts.contains_key(&contract_name) {
                if let Ok(contract) = json_document_to_contract(&path, true, platform_version) {
                    self.supporting_contracts.insert(contract_name, contract);
                }
            }
        }
    }

    fn update_supporting_contracts_sync(&mut self) {
        // Use block_in_place to wait for the async operation to complete
        tokio::task::block_in_place(|| {
            // Create a new Tokio runtime for the async operation
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                self.update_supporting_contracts().await;
            })
        });
    }
}

impl ScreenController for OperationsScreenController {
    fn name(&self) -> &'static str {
        "Operations"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        COMMAND_KEYS.as_ref()
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
            }) => {
                if let Some(strategy_name) = self.strategy_name.clone() {
                    // Update known contracts before showing the form
                    self.update_supporting_contracts_sync();

                    ScreenFeedback::Form(Box::new(StrategyAddOperationFormController::new(
                        strategy_name.clone(),
                        self.known_contracts.clone(),
                        self.supporting_contracts.clone(),
                    )))
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Task {
                task: Task::Strategy(StrategyTask::RemoveLastOperation(
                    self.strategy_name.clone().unwrap(),
                )),
                block: false,
            },
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
                self.selected_strategy = Some((*strategy).clone());
                self.strategy_name = Some(strategy_name.clone());
                self.contracts_with_updates = strategy.contracts_with_updates.clone();

                // Update the strategy_contract_names map
                if let Some(strategy_name) = &self.strategy_name {
                    self.strategy_contract_names
                        .insert(strategy_name.clone(), contract_names.to_vec());
                }

                ScreenFeedback::Redraw
            }
            _ => ScreenFeedback::None,
        }
    }

    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let display_text = if let Some(strategy) = &self.selected_strategy {
            let mut operations_lines = String::new();
            for op in &strategy.operations {
                let op_name = format_operation_name(&op.op_type);
                let times_per_block_display = if op.frequency.times_per_block_range.end
                    > op.frequency.times_per_block_range.start
                {
                    op.frequency.times_per_block_range.end - 1
                } else {
                    op.frequency.times_per_block_range.end
                };
                operations_lines.push_str(&format!(
                    "{:indent$}{}; Times per block: {}, chance per block: {}\n",
                    "",
                    op_name,
                    times_per_block_display,
                    op.frequency.chance_per_block.unwrap_or(0.0),
                    indent = 0
                ));
            }

            if operations_lines.is_empty() {
                "No operations defined for this strategy.".to_string()
            } else {
                format!(
                    "Strategy: {}\nOperations:\n{}",
                    self.strategy_name
                        .as_ref()
                        .unwrap_or(&"Unknown".to_string()),
                    operations_lines
                )
            }
        } else {
            "Select a strategy to view its operations.".to_string()
        };

        self.info = Info::new_fixed(&display_text);
        self.info.view(frame, area);
    }
}

// Helper function to format the operation name
fn format_operation_name(op_type: &OperationType) -> String {
    match op_type {
        OperationType::Document(op) => {
            let op_type = match op.action {
                DocumentAction::DocumentActionInsertRandom(..) => "InsertRandom",
                DocumentAction::DocumentActionDelete => "Delete",
                DocumentAction::DocumentActionReplace => "Replace",
                _ => "Unknown",
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
        OperationType::ContractUpdate(op) => match op.action {
            DataContractNewDocumentTypes(_) => format!(
                "ContractUpdate(NewDocTypesRandom): Contract: {}",
                op.contract.id().to_string(Encoding::Base58)
            ),
            DataContractNewOptionalFields(..) => format!(
                "ContractUpdate(NewFieldsRandom): Contract: {}",
                op.contract.id().to_string(Encoding::Base58)
            ),
        }
        .to_string(),
        OperationType::IdentityTransfer => "IdentityTransfer".to_string(),
        // Add other operation types as necessary
    }
}
