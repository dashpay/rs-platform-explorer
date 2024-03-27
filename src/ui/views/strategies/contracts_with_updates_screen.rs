//! Edit contracts_with_updates screen.

use std::collections::BTreeMap;

use dpp::{
    data_contract::{created_data_contract::CreatedDataContract, DataContract},
    tests::json_document::json_document_to_contract,
    version::PlatformVersion,
};
use strategy_tests::Strategy;
use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};
use walkdir::WalkDir;

use super::contracts_with_updates::{
    ContractsWithUpdatesFormController, RandomContractsFormController,
};
use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent, StrategyTask, Task},
    ui::screen::{
        utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
        ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

const COMMAND_KEYS: [ScreenCommandKey; 5] = [
    ScreenCommandKey::new("q", "Back to Strategy"),
    ScreenCommandKey::new("s", "Add specific"),
    ScreenCommandKey::new("x", "Add x random"),
    ScreenCommandKey::new("r", "Remove last"),
    ScreenCommandKey::new("c", "Clear all"),
];

pub(crate) struct ContractsWithUpdatesScreenController {
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

impl_builder!(ContractsWithUpdatesScreenController);

impl ContractsWithUpdatesScreenController {
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

impl ScreenController for ContractsWithUpdatesScreenController {
    fn name(&self) -> &'static str {
        "Contracts with updates"
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

                    ScreenFeedback::Form(Box::new(ContractsWithUpdatesFormController::new(
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
        let display_text = if let Some(strategy_name) = &self.strategy_name {
            if let Some(contracts_with_updates) = self.strategy_contract_names.get(strategy_name) {
                if contracts_with_updates.is_empty() {
                    "No contracts_with_updates".to_string()
                } else {
                    let mut contracts_with_updates_lines = String::new();
                    contracts_with_updates_lines
                        .push_str(&format!("Strategy: {}\n", strategy_name));
                    contracts_with_updates_lines.push_str("Contracts with updates:\n");
                    for (contract_name, updates) in contracts_with_updates {
                        contracts_with_updates_lines.push_str(&format!(
                            "{:indent$}Contract: {}\n",
                            "",
                            contract_name,
                            indent = 0
                        ));
                        if let Some(updates_map) = updates {
                            for (block, update) in updates_map {
                                contracts_with_updates_lines.push_str(&format!(
                                    "{:indent$}On block {} apply {}\n",
                                    "",
                                    block * 3,
                                    update,
                                    indent = 4
                                ));
                            }
                        }
                    }
                    contracts_with_updates_lines
                }
            } else {
                "Contracts with updates not found for selected strategy.".to_string()
            }
        } else {
            "Select a strategy to view contracts_with_updates.".to_string()
        };

        self.info = Info::new_fixed(&display_text);
        self.info.view(frame, area);
    }
}
