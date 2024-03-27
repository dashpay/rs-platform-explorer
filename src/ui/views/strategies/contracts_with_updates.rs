//! Strategy's contracts with updates form.

use std::collections::BTreeMap;

use rs_sdk::platform::DataContract;
use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{StrategyTask, Task},
    ui::form::{ComposedInput, Field, FormController, FormStatus, Input, InputStatus, SelectInput},
};

pub(super) struct ContractsWithUpdatesFormController {
    selected_strategy: String,
    known_contracts: BTreeMap<String, DataContract>,
    supporting_contracts: BTreeMap<String, DataContract>,
    selected_contracts: Vec<String>,
    input: ComposedInput<(Field<SelectInput<String>>, Field<SelectInput<String>>)>,
}

impl ContractsWithUpdatesFormController {
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
            selected_contracts: Vec::new(),
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

impl FormController for ContractsWithUpdatesFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((selected_contract, add_another_answer)) => {
                self.selected_contracts.push(selected_contract);

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
                        task: Task::Strategy(StrategyTask::SetContractsWithUpdates(
                            self.selected_strategy.clone(),
                            self.selected_contracts.clone(),
                        )),
                        block: false,
                    }
                }
            }
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Contract with updates for strategy"
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
    input: ComposedInput<(Field<SelectInput<String>>, Field<SelectInput<u8>>)>,
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
                    "Select the number of variants to create",
                    SelectInput::new(vec![5, 10, 20, 50, 100]),
                ),
            )),
        }
    }
}

impl FormController for RandomContractsFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((selected_contract_name, variants)) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::SetContractsWithUpdatesRandom(
                    self.selected_strategy.clone(),
                    selected_contract_name,
                    variants,
                )),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Random initial contracts for strategy"
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
