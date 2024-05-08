//! Screens and forms related to strategies manipulation.

mod identity_inserts;
mod operations;
mod run_strategy;
pub mod selected_strategy;
mod start_contracts;
mod start_identities;

use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

use self::selected_strategy::SelectedStrategyScreenController;
use crate::{
    backend::{AppState, AppStateUpdate, BackendEvent},
    ui::screen::{
        utils::impl_builder, widgets::info::Info, ScreenCommandKey, ScreenController,
        ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

use crate::{
    backend::{StrategyTask, Task},
    ui::form::{
        parsers::DefaultTextInputParser, FormController, FormStatus, Input, InputStatus,
        SelectInput, TextInput,
    },
};

const COMMAND_KEYS: [ScreenCommandKey; 6] = [
    ScreenCommandKey::new("q", "Back to Main"),
    ScreenCommandKey::new("n", "New strategy"),
    ScreenCommandKey::new("i", "Import a strategy"),
    ScreenCommandKey::new("e", "Export a strategy"),
    ScreenCommandKey::new("s", "Select a strategy"),
    ScreenCommandKey::new("d", "Delete a strategy"),
];

pub(crate) struct StrategiesScreenController {
    info: Info,
    available_strategies: Vec<String>,
    selected_strategy: Option<String>,
}

impl_builder!(StrategiesScreenController);

impl StrategiesScreenController {
    pub(crate) async fn new(app_state: &AppState) -> Self {
        let available_strategies_lock = app_state.available_strategies.lock().await;
        let strategies = available_strategies_lock
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let info_text = if strategies.is_empty() {
            "No available strategies".to_string()
        } else {
            "Strategy management commands".to_string()
        };

        let info = Info::new_fixed(&info_text);

        StrategiesScreenController {
            info,
            available_strategies: strategies,
            selected_strategy: None,
        }
    }
}

impl ScreenController for StrategiesScreenController {
    fn name(&self) -> &'static str {
        "Strategies"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        if self.available_strategies.is_empty() {
            &COMMAND_KEYS[..3] // Exclude certain operations when there are no available strategies
        } else {
            COMMAND_KEYS.as_ref()
        }
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
                code: Key::Char('n'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::FormThenNextScreen {
                form: Box::new(NewStrategyFormController::new()),
                screen: SelectedStrategyScreenController::builder(),
            },
            Event::Key(KeyEvent {
                code: Key::Char('i'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::FormThenNextScreen {
                form: Box::new(ImportStrategyFormController::new()),
                screen: SelectedStrategyScreenController::builder(),
            },
            Event::Key(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if !self.available_strategies.is_empty() {
                    self.info =
                        Info::new_fixed(">Exported strategy to supporting_files/strategy_exports");

                    ScreenFeedback::Form(Box::new(ExportStrategyFormController::new(
                        self.available_strategies.clone(),
                    )))
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('s'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if !self.available_strategies.is_empty() {
                    ScreenFeedback::FormThenNextScreen {
                        form: Box::new(SelectStrategyFormController::new(
                            self.available_strategies.clone(),
                        )),
                        screen: SelectedStrategyScreenController::builder(),
                    }
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => {
                if !self.available_strategies.is_empty() {
                    ScreenFeedback::Form(Box::new(DeleteStrategyFormController::new(
                        self.available_strategies.clone(),
                    )))
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Backend(
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name,
                    _strategy,
                    _contract_names,
                ))
                | BackendEvent::TaskCompletedStateChange {
                    app_state_update:
                        AppStateUpdate::SelectedStrategy(strategy_name, _strategy, _contract_names),
                    ..
                },
            ) => {
                self.selected_strategy = Some(strategy_name.clone());
                if !self.available_strategies.contains(&strategy_name) {
                    self.available_strategies.push(strategy_name.clone());
                }
                ScreenFeedback::Redraw
            }
            Event::Backend(
                BackendEvent::AppStateUpdated(AppStateUpdate::Strategies(strategies, ..)),
                ..,
            ) => {
                self.available_strategies = strategies.keys().cloned().collect();

                let info_text = if self.available_strategies.is_empty() {
                    "No available strategies".to_string()
                } else {
                    "Strategy management commands".to_string()
                };

                self.info = Info::new_fixed(&info_text);
                ScreenFeedback::Redraw
            }
            _ => ScreenFeedback::None,
        }
    }

    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.info.view(frame, area)
    }
}

pub(crate) struct NewStrategyFormController {
    input: TextInput<DefaultTextInputParser<String>>,
}

impl NewStrategyFormController {
    pub(crate) fn new() -> Self {
        NewStrategyFormController {
            input: TextInput::new("strategy name"),
        }
    }
}

impl FormController for NewStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(strategy_name) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::CreateStrategy(strategy_name)),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Create new strategy"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Strategy name"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

pub(crate) struct ImportStrategyFormController {
    input: TextInput<DefaultTextInputParser<String>>,
}

impl ImportStrategyFormController {
    pub(crate) fn new() -> Self {
        Self {
            input: TextInput::new("Raw Github file URL (ex: https://raw.githubusercontent.com/pauldelucia/dash-platform-strategy-tests/main/example)"),
        }
    }
}

impl FormController for ImportStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(url) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::ImportStrategy(url)),
                block: false,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Import strategy"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Url"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

pub(super) struct ExportStrategyFormController {
    input: SelectInput<String>,
}

impl ExportStrategyFormController {
    pub(super) fn new(strategies: Vec<String>) -> Self {
        Self {
            input: SelectInput::new(strategies),
        }
    }
}

impl FormController for ExportStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(strategy_name) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::ExportStrategy(strategy_name)),
                block: false,
            },
            InputStatus::Exit => FormStatus::Exit,
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Strategy export"
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

pub(super) struct SelectStrategyFormController {
    input: SelectInput<String>,
}

impl SelectStrategyFormController {
    pub(super) fn new(strategies: Vec<String>) -> Self {
        SelectStrategyFormController {
            input: SelectInput::new(strategies),
        }
    }
}

impl FormController for SelectStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(strategy_name) => FormStatus::Done {
                task: Task::Strategy(StrategyTask::SelectStrategy(strategy_name)),
                block: false,
            },
            InputStatus::Exit => FormStatus::Exit,
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Strategy selection"
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

pub(super) struct DeleteStrategyFormController {
    strategy_input: SelectInput<String>,
    confirm_input: SelectInput<String>,
    selected_strategy: Option<String>,
    step: u8,
}

impl DeleteStrategyFormController {
    pub(super) fn new(strategies: Vec<String>) -> Self {
        DeleteStrategyFormController {
            strategy_input: SelectInput::new(strategies),
            confirm_input: SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
            selected_strategy: None,
            step: 0,
        }
    }
}

impl FormController for DeleteStrategyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.step {
            0 => match self.strategy_input.on_event(event) {
                InputStatus::Done(strategy_name) => {
                    self.selected_strategy = Some(strategy_name);
                    self.step = 1;
                    FormStatus::Redraw
                }
                status => status.into(),
            },
            1 => match self.confirm_input.on_event(event) {
                InputStatus::Done(choice) => {
                    if choice == "Yes" {
                        FormStatus::Done {
                            task: Task::Strategy(StrategyTask::DeleteStrategy(
                                self.selected_strategy.clone().unwrap(),
                            )),
                            block: false,
                        }
                    } else {
                        FormStatus::Exit
                    }
                }
                status => status.into(),
            },
            _ => FormStatus::None,
        }
    }

    fn form_name(&self) -> &'static str {
        "Strategy deletion"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        match self.step {
            0 => self.strategy_input.view(frame, area),
            1 => self.confirm_input.view(frame, area),
            _ => {}
        }
    }

    fn step_name(&self) -> &'static str {
        match self.step {
            0 => "Select Strategy",
            1 => "Confirm Deletion",
            _ => "",
        }
    }

    fn step_index(&self) -> u8 {
        self.step
    }

    fn steps_number(&self) -> u8 {
        2
    }
}
