//! Start identities for strategy form.

use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{StrategyTask, Task},
    ui::form::{ComposedInput, Field, FormController, FormStatus, Input, InputStatus, SelectInput},
};

pub(super) struct StrategyStartIdentitiesFormController {
    input: ComposedInput<(
        Field<SelectInput<u8>>,
        Field<SelectInput<u8>>,
        Field<SelectInput<String>>,
    )>,
    selected_strategy: String,
}

impl StrategyStartIdentitiesFormController {
    pub(super) fn new(selected_strategy: String) -> Self {
        StrategyStartIdentitiesFormController {
            input: ComposedInput::new((
                Field::new(
                    "Number of identities",
                    SelectInput::new(vec![1, 2, 3, 5, 10, 100]),
                ),
                Field::new(
                    "Keys per identity",
                    SelectInput::new(vec![3, 4, 5, 10, 15, 20, 32]),
                ),
                Field::new(
                    "Add transfer key?",
                    SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
                ),
            )),
            selected_strategy,
        }
    }
}

impl FormController for StrategyStartIdentitiesFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((count, keys_count, add_transfer_key)) => {
                if add_transfer_key == "Yes" {
                    FormStatus::Done {
                        task: Task::Strategy(StrategyTask::SetStartIdentities {
                            strategy_name: self.selected_strategy.clone(),
                            count,
                            keys_count,
                            balance: 10_000_000,
                            add_transfer_key: true,
                        }),
                        block: false,
                    }
                } else {
                    FormStatus::Done {
                        task: Task::Strategy(StrategyTask::SetStartIdentities {
                            strategy_name: self.selected_strategy.clone(),
                            count,
                            keys_count,
                            balance: 10_000_000,
                            add_transfer_key: false,
                        }),
                        block: false,
                    }
                }
            }
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Identity inserts for strategy"
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
        3
    }
}

pub(super) struct StrategyStartIdentitiesBalanceFormController {
    input: SelectInput<u64>,
    selected_strategy: String,
}

impl StrategyStartIdentitiesBalanceFormController {
    pub(super) fn new(selected_strategy: String) -> Self {
        Self {
            input: SelectInput::new(vec![
                1_000_000,
                10_000_000,
                50_000_000,
                100_000_000,
                300_000_000,
                500_000_000,
                1_000_000_000,
                1_500_000_000,
                2_000_000_000,
            ]),
            selected_strategy,
        }
    }
}

impl FormController for StrategyStartIdentitiesBalanceFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(balance) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::SetStartIdentitiesBalance(
                    self.selected_strategy.clone(),
                    balance,
                )),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Set initial identities balances (100_000_000 duffs = 1 dash)"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        ""
    }

    fn step_index(&self) -> u8 {
        1
    }

    fn steps_number(&self) -> u8 {
        1
    }
}
