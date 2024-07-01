//! UI definitions related to identities.

use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

use crate::{
    backend::{identities::IdentityTask, BackendEvent, Task},
    ui::{
        form::{
            parsers::DefaultTextInputParser, ComposedInput, Field, FormController, FormStatus,
            Input, InputStatus, TextInput,
        },
        screen::{
            utils::impl_builder_no_args, widgets::info::Info, ScreenCommandKey, ScreenController,
            ScreenFeedback, ScreenToggleKey,
        },
    },
    Event,
};

const COMMAND_KEYS: [ScreenCommandKey; 4] = [
    ScreenCommandKey::new("q", "Back to Main"),
    ScreenCommandKey::new("i", "Get Identity by ID"),
    ScreenCommandKey::new("t", "Transfer credits"),
    ScreenCommandKey::new("r", "Register DPNS name"),
];

pub(crate) struct IdentitiesScreenController {
    toggle_keys: [ScreenToggleKey; 1],
    info: Info,
}

impl_builder_no_args!(IdentitiesScreenController);

impl IdentitiesScreenController {
    pub(crate) fn new() -> Self {
        IdentitiesScreenController {
            toggle_keys: [ScreenToggleKey::new("p", "with proof")],
            info: Info::new_fixed("Identity management commands"),
        }
    }
}

impl ScreenController for IdentitiesScreenController {
    fn name(&self) -> &'static str {
        "Identities"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        COMMAND_KEYS.as_ref()
    }

    fn toggle_keys(&self) -> &[ScreenToggleKey] {
        self.toggle_keys.as_ref()
    }

    fn on_event(&mut self, event: &Event) -> ScreenFeedback {
        match event {
            Event::Key(KeyEvent {
                code: Key::Char('q'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::PreviousScreen,

            Event::Key(KeyEvent {
                code: Key::Char('i'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(GetIdentityByIdFormController::new())),

            Event::Key(KeyEvent {
                code: Key::Char('t'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(TransferCreditsFormController::new())),

            Event::Key(KeyEvent {
                code: Key::Char('p'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.toggle_keys[0].toggle = !self.toggle_keys[0].toggle;
                ScreenFeedback::Redraw
            }

            Event::Key(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(RegisterDPNSNameFormController::new())),

            Event::Key(k) => {
                let redraw_info = self.info.on_event(k);
                if redraw_info {
                    ScreenFeedback::Redraw
                } else {
                    ScreenFeedback::None
                }
            }

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::FetchIdentityById(..),
                execution_result,
            }) => {
                self.info = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Identity(IdentityTask::TransferCredits(..)),
                execution_result,
            }) => {
                self.info = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompletedStateChange {
                task: Task::Identity(IdentityTask::TransferCredits(..)),
                execution_result,
                app_state_update: _,
            }) => {
                self.info = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompletedStateChange {
                task: Task::Identity(IdentityTask::RegisterDPNSName(..)),
                execution_result,
                app_state_update: _,
            }) => {
                self.info = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Identity(_),
                execution_result,
            }) => {
                self.info = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            _ => ScreenFeedback::None,
        }
    }

    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.info.view(frame, area)
    }
}

pub(crate) struct GetIdentityByIdFormController {
    input: TextInput<DefaultTextInputParser<String>>, // TODO: b58 parser
}

impl GetIdentityByIdFormController {
    fn new() -> Self {
        GetIdentityByIdFormController {
            input: TextInput::new("base58 id"),
        }
    }
}

impl FormController for GetIdentityByIdFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(value) => FormStatus::Done {
                task: Task::FetchIdentityById(value, false),
                block: true,
            },
            status => status.into(),
        }
    }

    fn step_view(&mut self, frame: &mut Frame, area: tuirealm::tui::prelude::Rect) {
        self.input.view(frame, area);
    }

    fn form_name(&self) -> &'static str {
        "Get identity by ID"
    }

    fn step_name(&self) -> &'static str {
        "Base 58 ID"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

pub(crate) struct TransferCreditsFormController {
    input: ComposedInput<(
        Field<TextInput<DefaultTextInputParser<String>>>,
        Field<TextInput<DefaultTextInputParser<f64>>>,
    )>,
}

impl TransferCreditsFormController {
    fn new() -> Self {
        Self {
            input: ComposedInput::new((
                Field::new("Enter the recipient base58 ID", TextInput::new("Base58 ID")),
                Field::new(
                    "Enter the amount to transfer in Dash (Ex: .5)",
                    TextInput::new("Amount to transfer in Dash"),
                ),
            )),
        }
    }
}

impl FormController for TransferCreditsFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done((recipient, amount)) => FormStatus::Done {
                task: Task::Identity(IdentityTask::TransferCredits(recipient, amount)),
                block: true,
            },
            status => status.into(),
        }
    }

    fn step_view(&mut self, frame: &mut Frame, area: tuirealm::tui::prelude::Rect) {
        self.input.view(frame, area);
    }

    fn form_name(&self) -> &'static str {
        "Transfer Credits"
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

pub(crate) struct RegisterDPNSNameFormController {
    input: TextInput<DefaultTextInputParser<String>>,
}

impl RegisterDPNSNameFormController {
    fn new() -> Self {
        RegisterDPNSNameFormController {
            input: TextInput::new(
                "DPNS name (example: enter \"something\" if you want \"something.dash\")",
            ),
        }
    }
}

impl FormController for RegisterDPNSNameFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(value) => FormStatus::Done {
                task: Task::Identity(IdentityTask::RegisterDPNSName(value)),
                block: true,
            },
            status => status.into(),
        }
    }

    fn step_view(&mut self, frame: &mut Frame, area: tuirealm::tui::prelude::Rect) {
        self.input.view(frame, area);
    }

    fn form_name(&self) -> &'static str {
        "Register DPNS Name"
    }

    fn step_name(&self) -> &'static str {
        "DPNS Name"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}
