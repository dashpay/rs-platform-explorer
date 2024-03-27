//! Strategy's identity inserts form.

use strategy_tests::frequency::Frequency;
use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{StrategyTask, Task},
    ui::form::{ComposedInput, Field, FormController, FormStatus, Input, InputStatus, SelectInput},
};

pub(super) struct StrategyIdentityInsertsFormController {
    input: ComposedInput<(Field<SelectInput<u16>>, Field<SelectInput<f64>>)>,
    selected_strategy: String,
}

impl StrategyIdentityInsertsFormController {
    pub(super) fn new(selected_strategy: String) -> Self {
        StrategyIdentityInsertsFormController {
            input: ComposedInput::new((
                Field::new(
                    "Times per block",
                    SelectInput::new(vec![1, 2, 5, 10, 20, 40, 100, 1000]),
                ),
                Field::new(
                    "Chance per block",
                    SelectInput::new(vec![1.0, 0.9, 0.75, 0.5, 0.25, 0.1, 0.05, 0.01]),
                ),
            )),
            selected_strategy,
        }
    }
}

impl FormController for StrategyIdentityInsertsFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((count, chance)) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::SetIdentityInserts {
                    strategy_name: self.selected_strategy.clone(),
                    identity_inserts_frequency: Frequency {
                        times_per_block_range: count..count + 1,
                        chance_per_block: Some(chance),
                    },
                }),
                block: false,
            },
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
        2
    }
}
