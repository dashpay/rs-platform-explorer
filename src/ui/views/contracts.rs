//! Contracts views.

pub mod document_type;
mod fetch_contract;
mod fetch_system_contract;

use std::{
    collections::BTreeMap,
    fmt::{self, Display},
};

use dpp::{
    data_contract::accessors::v0::DataContractV0Getters, platform_value::string_encoding::Encoding,
    prelude::DataContract,
};
use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

use self::{
    document_type::SelectDocumentTypeFormController, fetch_contract::FetchContractScreenController,
    fetch_system_contract::FetchSystemContractScreenController,
};
use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent, ContractTask, Task},
    ui::{
        form::{FormController, FormStatus, Input, InputStatus, SelectInput},
        screen::{
            utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
            ScreenFeedback, ScreenToggleKey,
        },
    },
    Event,
};

const COMMAND_KEYS: [ScreenCommandKey; 7] = [
    ScreenCommandKey::new("q", "Back to Main"),
    ScreenCommandKey::new("s", "Fetch system contract"),
    ScreenCommandKey::new("f", "Fetch contract"),
    ScreenCommandKey::new("↓ / C-n", "Next contract"),
    ScreenCommandKey::new("↑ / C-p", "Prev contract"),
    ScreenCommandKey::new("Enter", "Select contract"),
    ScreenCommandKey::new("r", "Remove a contract"),
];

/// Data contract name (identifier in app state) wrapper for better display
#[derive(Clone)]
struct DataContractEntry {
    name: String,
    id_b58: String,
    document_type_names: Vec<String>,
}

impl DataContractEntry {
    fn new(name: String, contract: &DataContract) -> Self {
        DataContractEntry {
            name,
            id_b58: contract.id_ref().to_string(Encoding::Base58),
            document_type_names: contract.document_types().keys().cloned().collect(),
        }
    }
}

impl Display for DataContractEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} ({} types)",
            self.name,
            self.id_b58,
            self.document_type_names.len(),
        )
    }
}

pub(crate) struct ContractsScreenController {
    select: Option<SelectInput<DataContractEntry>>,
    known_contracts: BTreeMap<String, DataContract>,
}

impl_builder!(ContractsScreenController);

impl ContractsScreenController {
    pub(crate) async fn new(app_state: &AppState) -> Self {
        let known_contracts_lock = app_state.known_contracts.lock().await;
        let select = if known_contracts_lock.len() > 0 {
            Some(SelectInput::new(Self::contract_entries_vec(
                known_contracts_lock.iter().map(|(k, v)| (k.clone(), v)),
            )))
        } else {
            None
        };
        let known_contracts = known_contracts_lock.clone();
        ContractsScreenController {
            select,
            known_contracts,
        }
    }

    fn contract_entries_vec<'a>(
        known_contracts: impl IntoIterator<Item = (String, &'a DataContract)>,
    ) -> Vec<DataContractEntry> {
        known_contracts
            .into_iter()
            .map(|(name, contract)| DataContractEntry::new(name, contract))
            .collect()
    }
}

impl ScreenController for ContractsScreenController {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(select) = &mut self.select {
            select.view(frame, area)
        } else {
            Info::new_fixed("No fetched data contracts").view(frame, area)
        }
    }

    fn name(&self) -> &'static str {
        "Contracts"
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
            }) => ScreenFeedback::NextScreen(FetchSystemContractScreenController::builder()),

            Event::Key(KeyEvent {
                code: Key::Char('f'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::NextScreen(FetchContractScreenController::builder()),

            Event::Key(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let contract_names = self
                    .known_contracts
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<String>>();

                ScreenFeedback::Form(Box::new(RemoveContractFormController::new(contract_names)))
            }

            Event::Key(event) => {
                if let Some(select) = &mut self.select {
                    match select.on_event(*event) {
                        InputStatus::Done(DataContractEntry {
                            name,
                            document_type_names,
                            ..
                        }) => ScreenFeedback::Form(Box::new(
                            SelectDocumentTypeFormController::new(name, document_type_names),
                        )),
                        InputStatus::Redraw => ScreenFeedback::Redraw,
                        _ => ScreenFeedback::None,
                    }
                } else {
                    ScreenFeedback::None
                }
            }

            Event::Backend(
                BackendEvent::AppStateUpdated(AppStateUpdate::KnownContracts(known_contracts))
                | BackendEvent::TaskCompletedStateChange {
                    app_state_update: AppStateUpdate::KnownContracts(known_contracts),
                    ..
                },
            ) => {
                self.select = if !known_contracts.is_empty() {
                    Some(SelectInput::new(Self::contract_entries_vec(
                        known_contracts.iter().map(|(k, v)| (k.clone(), v)),
                    )))
                } else {
                    None
                };
                self.known_contracts = (*known_contracts).clone();
                ScreenFeedback::Redraw
            }

            _ => ScreenFeedback::None,
        }
    }
}

pub(super) struct RemoveContractFormController {
    input: SelectInput<String>,
}

impl RemoveContractFormController {
    pub(super) fn new(contracts: Vec<String>) -> Self {
        Self {
            input: SelectInput::new(contracts),
        }
    }
}

impl FormController for RemoveContractFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(contract_name) => FormStatus::Done {
                task: Task::Contract(ContractTask::RemoveContract(contract_name)),
                block: false,
            },
            InputStatus::Exit => FormStatus::Exit,
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Remove contract"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        ""
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}
