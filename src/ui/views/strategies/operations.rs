//! Forms for operations management in strategy.

mod contract_create;
mod contract_update_doc_types;
mod contract_update_new_fields;
mod document;
mod identity_top_up;
mod identity_transfer;
mod identity_update;
mod identity_withdrawal;

use std::collections::BTreeMap;

use dash_sdk::platform::DataContract;
use dpp::data_contract::document_type::random_document::{
    DocumentFieldFillSize, DocumentFieldFillType,
};
use tracing::error;
use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use self::{
    contract_create::StrategyOpContractCreateFormController,
    contract_update_doc_types::StrategyOpContractUpdateDocTypesFormController,
    document::StrategyOpDocumentFormController,
    identity_top_up::StrategyOpIdentityTopUpFormController,
    identity_transfer::StrategyOpIdentityTransferFormController,
    identity_update::StrategyOpIdentityUpdateFormController,
    identity_withdrawal::StrategyOpIdentityWithdrawalFormController,
};
use crate::{
    backend::{StrategyContractNames, StrategyTask, Task},
    ui::form::{ComposedInput, Field, FormController, FormStatus, Input, InputStatus, SelectInput},
};

use dpp::{
    data_contract::{
        accessors::v0::DataContractV0Getters, created_data_contract::CreatedDataContract,
    },
    platform_value::string_encoding::Encoding,
    tests::json_document::json_document_to_contract,
    version::PlatformVersion,
};
use strategy_tests::{
    operations::{
        DataContractUpdateAction::{DataContractNewDocumentTypes, DataContractNewOptionalFields},
        DocumentAction, OperationType as StrategyOperationType,
    },
    Strategy,
};
use tuirealm::event::{Key, KeyModifiers};
use walkdir::WalkDir;

use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent},
    ui::screen::{
        utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
        ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

const COMMAND_KEYS: [ScreenCommandKey; 5] = [
    ScreenCommandKey::new("q", "Back to Strategy"),
    ScreenCommandKey::new("a", "Add"),
    ScreenCommandKey::new("r", "Remove last"),
    ScreenCommandKey::new("c", "Clear all"),
    ScreenCommandKey::new("x", "Register x documents to all contracts"),
];

pub struct OperationsScreenController {
    info: Info,
    strategy_name: Option<String>,
    selected_strategy: Option<Strategy>,
    start_contracts: Vec<(
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

        let (info_text, current_strategy, current_start_contracts) =
            if let Some(selected_strategy_name) = &*selected_strategy_lock {
                if let Some(strategy) = available_strategies_lock.get(selected_strategy_name) {
                    // Construct the info_text and get the start_contracts for the selected
                    // strategy
                    let info_text = format!("Selected Strategy: {}", selected_strategy_name);
                    let current_start_contracts = strategy.start_contracts.clone();
                    (info_text, Some(strategy.clone()), current_start_contracts)
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
            start_contracts: current_start_contracts,
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

                    let strategy_contract_names = self
                        .strategy_contract_names
                        .get(&strategy_name)
                        .expect("Expected to get strategy contract names in operations screen");

                    ScreenFeedback::Form(Box::new(StrategyAddOperationFormController::new(
                        strategy_name.clone(),
                        self.known_contracts.clone(),
                        self.supporting_contracts.clone(),
                        strategy_contract_names.to_vec(),
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
            Event::Key(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if let Some(strategy_name) = &self.strategy_name {
                    ScreenFeedback::Task {
                        task: Task::Strategy(StrategyTask::ClearOperations(strategy_name.clone())),
                        block: false,
                    }
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('x'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if let Some(strategy_name) = self.strategy_name.clone() {
                    ScreenFeedback::Form(Box::new(StrategyAutomaticDocumentsFormController::new(
                        strategy_name.clone(),
                    )))
                } else {
                    ScreenFeedback::None
                }
            }
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
                self.start_contracts = strategy.start_contracts.clone();

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
fn format_operation_name(op_type: &StrategyOperationType) -> String {
    match op_type {
        StrategyOperationType::Document(op) => {
            let op_type = match op.action {
                DocumentAction::DocumentActionInsertRandom(..) => "InsertRandom",
                DocumentAction::DocumentActionDelete => "Delete",
                // DocumentAction::DocumentActionReplace => "Replace",
                _ => "Unknown",
            };
            format!(
                "Document({}): Contract: {}",
                op_type,
                op.contract.id().to_string(Encoding::Base58)
            )
        }
        StrategyOperationType::IdentityTopUp => "IdentityTopUp".to_string(),
        StrategyOperationType::IdentityUpdate(op) => format!("IdentityUpdate({:?})", op),
        StrategyOperationType::IdentityWithdrawal => "IdentityWithdrawal".to_string(),
        StrategyOperationType::ContractCreate(..) => "ContractCreateRandom".to_string(),
        StrategyOperationType::ContractUpdate(op) => match op.action {
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
        StrategyOperationType::IdentityTransfer => "IdentityTransfer".to_string(),
        StrategyOperationType::ResourceVote(_) => "ResourceVote".to_string(),
    }
}

#[derive(Debug, strum::Display, Clone, strum::EnumIter, Copy)]
enum OperationType {
    Document,
    IdentityTopUp,
    IdentityAddKeys,
    IdentityDisableKeys,
    IdentityWithdrawal,
    IdentityTransfer,
    ContractCreateRandom,
    ContractUpdateDocTypesRandom,
    // ContractUpdateFieldsRandom,
}

pub(super) struct StrategyAddOperationFormController {
    op_type_input: SelectInput<String>,
    op_specific_form: Option<Box<dyn FormController>>,
    strategy_name: String,
    known_contracts: BTreeMap<String, DataContract>,
    supporting_contracts: BTreeMap<String, DataContract>,
    strategy_contract_names: StrategyContractNames,
}

impl StrategyAddOperationFormController {
    pub(super) fn new(
        strategy_name: String,
        known_contracts: BTreeMap<String, DataContract>,
        supporting_contracts: BTreeMap<String, DataContract>,
        strategy_contract_names: StrategyContractNames,
    ) -> Self {
        let operation_types = vec![
            "Document".to_string(),
            "IdentityTopUp".to_string(),
            "IdentityAddKeys".to_string(),
            "IdentityDisableKeys".to_string(),
            "IdentityWithdrawal".to_string(),
            "IdentityTransfer (requires start_identities > 0)".to_string(),
            "ContractCreateRandom".to_string(),
            "ContractUpdateDocTypesRandom".to_string(),
            // "ContractUpdateFieldsRandom".to_string(),
        ];
        Self {
            op_type_input: SelectInput::new(operation_types),
            op_specific_form: None,
            strategy_name,
            known_contracts,
            supporting_contracts,
            strategy_contract_names,
        }
    }

    fn set_op_form(&mut self, op_type: OperationType) {
        self.op_specific_form = Some(match op_type {
            OperationType::Document => Box::new(StrategyOpDocumentFormController::new(
                self.strategy_name.clone(),
                self.known_contracts.clone(),
                self.supporting_contracts.clone(),
                self.strategy_contract_names.clone(),
            )),
            OperationType::IdentityTopUp => Box::new(StrategyOpIdentityTopUpFormController::new(
                self.strategy_name.clone(),
            )),
            OperationType::IdentityAddKeys => {
                Box::new(StrategyOpIdentityUpdateFormController::new(
                    self.strategy_name.clone(),
                    identity_update::KeyUpdateOp::AddKeys,
                ))
            }
            OperationType::IdentityDisableKeys => {
                Box::new(StrategyOpIdentityUpdateFormController::new(
                    self.strategy_name.clone(),
                    identity_update::KeyUpdateOp::DisableKeys,
                ))
            }
            OperationType::IdentityWithdrawal => Box::new(
                StrategyOpIdentityWithdrawalFormController::new(self.strategy_name.clone()),
            ),
            OperationType::IdentityTransfer => Box::new(
                StrategyOpIdentityTransferFormController::new(self.strategy_name.clone()),
            ),
            OperationType::ContractCreateRandom => Box::new(
                StrategyOpContractCreateFormController::new(self.strategy_name.clone()),
            ),
            OperationType::ContractUpdateDocTypesRandom => {
                Box::new(StrategyOpContractUpdateDocTypesFormController::new(
                    self.strategy_name.clone(),
                    self.known_contracts.clone(),
                ))
            } /* OperationType::ContractUpdateFieldsRandom => Box::new(
               *     StrategyOpContractUpdateNewFieldsFormController::new(self.strategy_name.
               * clone()), ), */
        });
    }
}

impl FormController for StrategyAddOperationFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        if let Some(form) = &mut self.op_specific_form {
            form.on_event(event)
        } else {
            match self.op_type_input.on_event(event) {
                InputStatus::Done(op_type) => {
                    let operation_type = match op_type.as_str() {
                        "Document" => OperationType::Document,
                        "IdentityTopUp" => OperationType::IdentityTopUp,
                        "IdentityAddKeys" => OperationType::IdentityAddKeys,
                        "IdentityDisableKeys" => OperationType::IdentityDisableKeys,
                        "IdentityWithdrawal" => OperationType::IdentityWithdrawal,
                        "IdentityTransfer (requires start_identities > 0)" => {
                            OperationType::IdentityTransfer
                        }
                        "ContractCreateRandom" => OperationType::ContractCreateRandom,
                        "ContractUpdateDocTypesRandom" => {
                            OperationType::ContractUpdateDocTypesRandom
                        }
                        // "ContractUpdateFieldsRandom" => OperationType::ContractUpdateFields,
                        _ => {
                            error!("Non-existant operation type selected");
                            panic!("Non-existant operation type selected")
                        }
                    };
                    self.set_op_form(operation_type);
                    FormStatus::Redraw
                }
                status => status.into(),
            }
        }
    }

    fn form_name(&self) -> &'static str {
        if let Some(form) = &self.op_specific_form {
            form.form_name()
        } else {
            "Add strategy operation"
        }
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(form) = &mut self.op_specific_form {
            form.step_view(frame, area)
        } else {
            self.op_type_input.view(frame, area)
        }
    }

    fn step_name(&self) -> &'static str {
        if let Some(form) = &self.op_specific_form {
            form.step_name()
        } else {
            "Select operation"
        }
    }

    fn step_index(&self) -> u8 {
        if let Some(form) = &self.op_specific_form {
            form.step_index()
        } else {
            0
        }
    }

    fn steps_number(&self) -> u8 {
        if let Some(form) = &self.op_specific_form {
            form.steps_number()
        } else {
            1
        }
    }
}

pub(super) struct StrategyAutomaticDocumentsFormController {
    strategy_name: String,
    input: ComposedInput<(
        Field<SelectInput<u16>>,    // Num documents
        Field<SelectInput<String>>, // Fill size
        Field<SelectInput<String>>, // Fill type
    )>,
}

impl StrategyAutomaticDocumentsFormController {
    pub(super) fn new(strategy_name: String) -> Self {
        let num_docs = vec![1, 3, 5, 10, 15, 20, 24];
        Self {
            strategy_name,
            input: ComposedInput::new((
                Field::new(
                    "Select number of docs to add to each contract",
                    SelectInput::new(num_docs),
                ),
                Field::new(
                    "How much data to populate the document with?",
                    SelectInput::new(vec![
                        "Minimum".to_string(),
                        "Maximum".to_string(),
                        "Random".to_string(),
                    ]),
                ),
                Field::new(
                    "Populate not-required fields?",
                    SelectInput::new(vec!["No".to_string(), "Yes".to_string()]),
                ),
            )),
        }
    }
}

impl FormController for StrategyAutomaticDocumentsFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((num_docs, fill_size_string, fill_type_string)) => {
                let fill_size = match &fill_size_string as &str {
                    "Minimum" => DocumentFieldFillSize::MinDocumentFillSize,
                    "Maximum" => DocumentFieldFillSize::MaxDocumentFillSize,
                    "Random" => DocumentFieldFillSize::AnyDocumentFillSize,
                    _ => {
                        tracing::error!("Fill size string invalid in document creation. Setting to AnyDocumentFillSize.");
                        DocumentFieldFillSize::AnyDocumentFillSize
                    }
                };
                let fill_type = match &fill_type_string as &str {
                    "Yes" => DocumentFieldFillType::FillIfNotRequired,
                    "No" => DocumentFieldFillType::DoNotFillIfNotRequired,
                    _ => {
                        tracing::error!("Fill size string invalid in document creation. Setting to DoNotFillIfNotRequired.");
                        DocumentFieldFillType::DoNotFillIfNotRequired
                    }
                };

                FormStatus::Done {
                    task: Task::Strategy(StrategyTask::RegisterDocsToAllContracts(
                        self.strategy_name.clone(),
                        num_docs,
                        fill_size,
                        fill_type,
                    )),
                    block: false,
                }
            }
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Auto add docs to contracts"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        self.input.step_name()
    }

    fn step_index(&self) -> u8 {
        self.input.step_index()
    }

    fn steps_number(&self) -> u8 {
        self.input.steps_number()
    }
}
