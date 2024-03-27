//! Run strategy confirmation form.

use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{StrategyTask, Task},
    ui::form::{ComposedInput, Field, FormController, FormStatus, Input, InputStatus, SelectInput},
};

pub(super) struct RunStrategyFormController {
    input: ComposedInput<(
        Field<SelectInput<String>>,
        Field<SelectInput<u64>>,
        Field<SelectInput<String>>,
        Field<SelectInput<String>>,
    )>,
    selected_strategy: String,
}

impl RunStrategyFormController {
    pub(super) fn new(selected_strategy: String) -> Self {
        RunStrategyFormController {
            input: ComposedInput::new((
                Field::new(
                    "Execute strategy per block or per second?",
                    SelectInput::new(vec!["Block".to_string(), "Second".to_string()]),
                ),
                Field::new(
                    "Number of blocks or seconds to run the strategy",
                    SelectInput::new(vec![10, 20, 30, 60, 120, 300, 600]),
                ),
                Field::new(
                    "Verify state transition proofs? (Only applies to block mode)",
                    SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
                ),
                Field::new(
                    "Confirm you would like to run the strategy",
                    SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
                ),
            )),
            selected_strategy,
        }
    }
}

impl FormController for RunStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((mode, num_blocks, verify_proofs, confirm)) => {
                if confirm == "Yes" {
                    if verify_proofs == "Yes" {
                        if mode == "Block" {
                            // block-based with proofs
                            FormStatus::Done {
                                task: Task::Strategy(StrategyTask::RunStrategy(
                                    self.selected_strategy.clone(),
                                    num_blocks,
                                    true,
                                    true,
                                )),
                                block: true,
                            }
                        } else {
                            // time-based with proofs
                            FormStatus::Done {
                                task: Task::Strategy(StrategyTask::RunStrategy(
                                    self.selected_strategy.clone(),
                                    num_blocks,
                                    true,
                                    false,
                                )),
                                block: true,
                            }
                        }
                    } else {
                        if mode == "Block" {
                            // block-based without proofs
                            FormStatus::Done {
                                task: Task::Strategy(StrategyTask::RunStrategy(
                                    self.selected_strategy.clone(),
                                    num_blocks,
                                    false,
                                    true,
                                )),
                                block: true,
                            }
                        } else {
                            // time-based without proofs
                            FormStatus::Done {
                                task: Task::Strategy(StrategyTask::RunStrategy(
                                    self.selected_strategy.clone(),
                                    num_blocks,
                                    false,
                                    false,
                                )),
                                block: true,
                            }
                        }
                    }
                } else {
                    FormStatus::Exit
                }
            }
            InputStatus::Redraw => FormStatus::Redraw,
            InputStatus::None => FormStatus::None,
            InputStatus::Exit => FormStatus::Exit,
        }
    }

    fn form_name(&self) -> &'static str {
        "Run strategy"
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
        4
    }
}
