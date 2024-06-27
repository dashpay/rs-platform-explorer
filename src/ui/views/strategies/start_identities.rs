//! Start identities screen and forms.

use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

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

use strategy_tests::Strategy;

const COMMAND_KEYS: [ScreenCommandKey; 4] = [
    ScreenCommandKey::new("q", "Back to Strategy"),
    ScreenCommandKey::new("a", "Add/edit"),
    ScreenCommandKey::new("b", "Set balance"),
    ScreenCommandKey::new("c", "Remove"),
];

pub(crate) struct StartIdentitiesScreenController {
    info: Info,
    strategy_name: Option<String>,
    selected_strategy: Option<Strategy>,
}

impl_builder!(StartIdentitiesScreenController);

impl StartIdentitiesScreenController {
    pub(crate) async fn new(app_state: &AppState) -> Self {
        let available_strategies_lock = app_state.available_strategies.lock().await;
        let selected_strategy_lock = app_state.selected_strategy.lock().await;

        let (info_text, current_strategy) =
            if let Some(selected_strategy_name) = &*selected_strategy_lock {
                if let Some(strategy) = available_strategies_lock.get(selected_strategy_name) {
                    let info_text = format!("Selected Strategy: {}", selected_strategy_name);
                    (info_text, Some(strategy.clone()))
                } else {
                    ("No selected strategy found".to_string(), None)
                }
            } else {
                ("No strategy selected".to_string(), None)
            };

        let info = Info::new_fixed(&info_text);

        Self {
            info,
            strategy_name: selected_strategy_lock.clone(),
            selected_strategy: current_strategy,
        }
    }
}

impl ScreenController for StartIdentitiesScreenController {
    fn name(&self) -> &'static str {
        "Start identities"
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
                if let Some(strategy_name) = &self.strategy_name {
                    ScreenFeedback::Form(Box::new(StrategyStartIdentitiesFormController::new(
                        strategy_name.clone(),
                    )))
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if let Some(strategy_name) = &self.strategy_name {
                    ScreenFeedback::Form(Box::new(
                        StrategyStartIdentitiesBalanceFormController::new(strategy_name.clone()),
                    ))
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Task {
                task: Task::Strategy(StrategyTask::RemoveStartIdentities(
                    self.strategy_name.clone().unwrap(),
                )),
                block: false,
            },
            Event::Backend(BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                strategy_name,
                strategy,
                _,
            ))) => {
                // Check if the updated strategy is the one currently being displayed
                if Some(strategy_name) == self.strategy_name.as_ref() {
                    // Update the selected_strategy with the new data
                    self.selected_strategy = Some((*strategy).clone());

                    // Trigger a redraw of the screen
                    ScreenFeedback::Redraw
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Backend(BackendEvent::TaskCompletedStateChange {
                task: Task::Strategy(StrategyTask::SetStartIdentities { .. }),
                app_state_update:
                    AppStateUpdate::SelectedStrategy(strategy_name, updated_strategy, _),
                ..
            }) => {
                // Check if the updated strategy is the one currently being displayed
                if Some(&strategy_name) == self.strategy_name.as_ref().as_ref() {
                    // Update the selected_strategy with the new data
                    self.selected_strategy = Some((*updated_strategy).clone());

                    // Trigger a redraw of the screen
                    ScreenFeedback::Redraw
                } else {
                    ScreenFeedback::None
                }
            }
            _ => ScreenFeedback::None,
        }
    }

    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let display_text = if let Some(strategy) = &self.selected_strategy {
            // Construct the text to display start identities
            let start_identities_text = format!(
                "Start identities: {} (Keys: {}, Balance: {:.2} dash)",
                strategy.start_identities.number_of_identities,
                strategy.start_identities.keys_per_identity
                    + strategy.start_identities.extra_keys.len() as u8,
                strategy.start_identities.starting_balances as f64 / 100_000_000.0,
            );

            format!(
                "Strategy: {}\n{}",
                self.strategy_name
                    .as_ref()
                    .unwrap_or(&"Unknown".to_string()),
                start_identities_text
            )
        } else {
            "Select a strategy to view start identities.".to_string()
        };

        self.info = Info::new_fixed(&display_text);
        self.info.view(frame, area);
    }
}

pub(super) struct StrategyStartIdentitiesFormController {
    input: ComposedInput<(
        Field<TextInput<DefaultTextInputParser<u16>>>,
        Field<TextInput<DefaultTextInputParser<u8>>>,
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
                    TextInput::new("Enter a whole number"),
                ),
                Field::new(
                    "Keys per identity (min 3, max 32)",
                    TextInput::new("Enter a whole number"),
                ),
                Field::new(
                    "Add transfer key?",
                    SelectInput::new(vec!["No".to_string(), "Yes".to_string()]),
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
        "Start identities for strategy"
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
    input: TextInput<DefaultTextInputParser<f64>>,
    selected_strategy: String,
}

impl StrategyStartIdentitiesBalanceFormController {
    pub(super) fn new(selected_strategy: String) -> Self {
        Self {
            input: TextInput::new("Quantity (in Dash)"),
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
                    (balance * 100000000.0) as u64,
                )),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Set start identities balances"
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
