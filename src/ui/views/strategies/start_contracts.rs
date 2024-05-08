//! Start contracts screen and forms.

use std::collections::BTreeMap;

use walkdir::WalkDir;

use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent, StrategyTask, Task},
    ui::form::{
        parsers::DefaultTextInputParser, ComposedInput, Field, FormController, FormStatus, Input,
        InputStatus, SelectInput, TextInput,
    },
    ui::screen::{
        utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
        ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

use dash_sdk::platform::DataContract;
use dpp::{
    data_contract::created_data_contract::CreatedDataContract,
    tests::json_document::json_document_to_contract, version::PlatformVersion,
};
use strategy_tests::Strategy;

const COMMAND_KEYS: [ScreenCommandKey; 5] = [
    ScreenCommandKey::new("q", "Back to Strategy"),
    ScreenCommandKey::new("s", "Add specific"),
    ScreenCommandKey::new("x", "Add x random"),
    ScreenCommandKey::new("r", "Remove last"),
    ScreenCommandKey::new("c", "Clear all"),
];

pub(crate) struct StartContractsScreenController {
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

impl_builder!(StartContractsScreenController);

impl StartContractsScreenController {
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
                match json_document_to_contract(&path, true, platform_version) {
                    Ok(contract) => {
                        self.supporting_contracts.insert(contract_name, contract);
                    },
                    Err(e) => tracing::warn!("Unable to convert {contract_name} from json document to contract. Error: {e}"),
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

impl ScreenController for StartContractsScreenController {
    fn name(&self) -> &'static str {
        "Start contracts"
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
                code: Key::Char('s'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let strategy_name_clone = self.strategy_name.clone();
                if let Some(strategy_name) = strategy_name_clone {
                    self.update_supporting_contracts_sync();

                    ScreenFeedback::Form(Box::new(StartContractsFormController::new(
                        strategy_name,
                        self.known_contracts.clone(),
                        self.supporting_contracts.clone(),
                    )))
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('x'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let strategy_name_clone = self.strategy_name.clone();
                if let Some(strategy_name) = strategy_name_clone {
                    self.update_supporting_contracts_sync();

                    ScreenFeedback::Form(Box::new(RandomContractsFormController::new(
                        strategy_name,
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
            }) => {
                if let Some(strategy_name) = &self.strategy_name {
                    ScreenFeedback::Task {
                        task: Task::Strategy(StrategyTask::RemoveLastContract(
                            strategy_name.clone(),
                        )),
                        block: false,
                    }
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if let Some(strategy_name) = &self.strategy_name {
                    ScreenFeedback::Task {
                        task: Task::Strategy(StrategyTask::ClearContracts(strategy_name.clone())),
                        block: false,
                    }
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
        let display_text = if let Some(strategy_name) = &self.strategy_name {
            if let Some(start_contracts) = self.strategy_contract_names.get(strategy_name) {
                if start_contracts.is_empty() {
                    "No start_contracts".to_string()
                } else {
                    let mut start_contracts_lines = String::new();
                    start_contracts_lines.push_str(&format!("Strategy: {}\n", strategy_name));
                    start_contracts_lines.push_str("Start contracts:\n");
                    for (contract_name, updates) in start_contracts {
                        start_contracts_lines.push_str(&format!(
                            "{:indent$}Contract: {}\n",
                            "",
                            contract_name,
                            indent = 0
                        ));
                        if let Some(updates_map) = updates {
                            for (block, update) in updates_map {
                                start_contracts_lines.push_str(&format!(
                                    "{:indent$}On block {} apply {}\n",
                                    "",
                                    block * 3,
                                    update,
                                    indent = 4
                                ));
                            }
                        }
                    }
                    start_contracts_lines
                }
            } else {
                "Start contracts not found for selected strategy.".to_string()
            }
        } else {
            "Select a strategy to view start_contracts.".to_string()
        };

        self.info = Info::new_fixed(&display_text);
        self.info.view(frame, area);
    }
}

pub(super) struct StartContractsFormController {
    selected_strategy: String,
    known_contracts: BTreeMap<String, DataContract>,
    supporting_contracts: BTreeMap<String, DataContract>,
    selected_contract_names: Vec<String>,
    input: ComposedInput<(Field<SelectInput<String>>, Field<SelectInput<String>>)>,
}

impl StartContractsFormController {
    pub(super) fn new(
        selected_strategy: String,
        known_contracts: BTreeMap<String, DataContract>,
        supporting_contracts: BTreeMap<String, DataContract>,
    ) -> Self {
        // Collect contract names from known_contracts
        let mut contract_names: Vec<String> = known_contracts.keys().cloned().collect();

        // Collect and add names from supporting_contracts, avoiding duplicates
        let additional_names: Vec<String> = supporting_contracts
            .keys()
            .filter(|name| !contract_names.contains(name))
            .cloned()
            .collect();
        contract_names.extend(additional_names);

        // Remove duplicates
        contract_names.sort();
        contract_names.dedup();

        Self {
            selected_strategy,
            known_contracts,
            supporting_contracts,
            selected_contract_names: Vec::new(),
            input: ComposedInput::new((
                Field::new("Select Contract", SelectInput::new(contract_names)),
                Field::new(
                    "Add Another Contract? Note only compatible contract updates will actually \
                     work.",
                    SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
                ),
            )),
        }
    }
}

impl FormController for StartContractsFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((selected_contract, add_another_answer)) => {
                self.selected_contract_names.push(selected_contract);

                if add_another_answer == "Yes" {
                    // Collect contract names from known_contracts
                    let mut contract_names: Vec<String> =
                        self.known_contracts.keys().cloned().collect();

                    // Collect and add names from supporting_contracts, avoiding duplicates
                    let additional_names: Vec<String> = self
                        .supporting_contracts
                        .keys()
                        .filter(|name| !contract_names.contains(name))
                        .cloned()
                        .collect();
                    contract_names.extend(additional_names);

                    // Remove duplicates
                    contract_names.sort();
                    contract_names.dedup();

                    self.input = ComposedInput::new((
                        Field::new("Select Contract", SelectInput::new(contract_names)),
                        Field::new(
                            "Add Another Contract? Note only compatible contract updates will \
                             actually work.",
                            SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
                        ),
                    ));
                    FormStatus::Redraw
                } else {
                    FormStatus::Done {
                        task: Task::Strategy(StrategyTask::SetStartContracts(
                            self.selected_strategy.clone(),
                            self.selected_contract_names.clone(),
                        )),
                        block: false,
                    }
                }
            }
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Start contracts for strategy"
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
        2
    }
}

pub(super) struct RandomContractsFormController {
    selected_strategy: String,
    input: ComposedInput<(
        Field<SelectInput<String>>,
        Field<TextInput<DefaultTextInputParser<u8>>>,
    )>,
}

impl RandomContractsFormController {
    pub(super) fn new(
        selected_strategy: String,
        known_contracts: BTreeMap<String, DataContract>,
        supporting_contracts: BTreeMap<String, DataContract>,
    ) -> Self {
        // Collect contract names from known_contracts
        let mut contract_names: Vec<String> = known_contracts.keys().cloned().collect();

        // Collect and add names from supporting_contracts, avoiding duplicates
        let additional_names: Vec<String> = supporting_contracts
            .keys()
            .filter(|name| !contract_names.contains(name))
            .cloned()
            .collect();
        contract_names.extend(additional_names);

        // Remove duplicates
        contract_names.sort();
        contract_names.dedup();

        Self {
            selected_strategy,
            input: ComposedInput::new((
                Field::new(
                    "Select a contract for the random variants to be based off of",
                    SelectInput::new(contract_names),
                ),
                Field::new(
                    "Number of contract variants",
                    TextInput::new("Enter a whole number"),
                ),
            )),
        }
    }
}

impl FormController for RandomContractsFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((selected_contract_name, variants_count)) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::SetStartContractsRandom(
                    self.selected_strategy.clone(),
                    selected_contract_name,
                    variants_count,
                )),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Random start contracts for strategy"
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
        2
    }
}
