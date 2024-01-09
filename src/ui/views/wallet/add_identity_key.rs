//! Definition of a form to add a key to an identity.

use dpp::identity::{KeyType, Purpose as KeyPurpose, SecurityLevel as KeySecurityLevel};
use strum::IntoEnumIterator;
use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{identities::IdentityTask, Task},
    ui::form::{FormController, FormStatus, Input, InputStatus, SelectInput},
};

enum AddIdentityKeyFormStep {
    Purpose(SelectInput<KeyPurpose>),
    Security(SelectInput<KeySecurityLevel>),
    KeyType(SelectInput<KeyType>),
}

pub(super) struct AddIdentityKeyFormController {
    step: AddIdentityKeyFormStep,
    purpose_result: Option<KeyPurpose>,
    security_result: Option<KeySecurityLevel>,
}

impl AddIdentityKeyFormController {
    pub fn new() -> Self {
        AddIdentityKeyFormController {
            step: AddIdentityKeyFormStep::Purpose(SelectInput::new(vec![
                KeyPurpose::AUTHENTICATION,
                KeyPurpose::ENCRYPTION,
                KeyPurpose::DECRYPTION,
                KeyPurpose::WITHDRAW,
            ])),
            purpose_result: None,
            security_result: None,
        }
    }
}

impl FormController for AddIdentityKeyFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match &mut self.step {
            AddIdentityKeyFormStep::Purpose(input) => match input.on_event(event) {
                // En/Decryption keys form adjustments
                InputStatus::Done(purpose @ (KeyPurpose::ENCRYPTION | KeyPurpose::DECRYPTION)) => {
                    self.purpose_result = Some(purpose);
                    // En/Decryption keys have medium security so the field will be skipped
                    self.security_result = Some(KeySecurityLevel::MEDIUM);
                    // For En/Decryption keys we allow are ECDSA_SECP256K1 and BLS12_381 only
                    self.step = AddIdentityKeyFormStep::KeyType(SelectInput::new(vec![
                        KeyType::ECDSA_SECP256K1,
                        KeyType::BLS12_381,
                    ]));
                    FormStatus::Redraw
                }
                // Withdraw keys form adjustments
                InputStatus::Done(purpose @ KeyPurpose::WITHDRAW) => {
                    self.purpose_result = Some(purpose);
                    // Withdraw keys have critical security so the field will be skipped
                    self.security_result = Some(KeySecurityLevel::CRITICAL);
                    self.step = AddIdentityKeyFormStep::KeyType(SelectInput::new(
                        KeyType::iter().collect(),
                    ));
                    FormStatus::Redraw
                }
                // Authentication keys
                InputStatus::Done(purpose) => {
                    self.purpose_result = Some(purpose);
                    self.step = AddIdentityKeyFormStep::Security(SelectInput::new(
                        KeySecurityLevel::iter().collect(),
                    ));
                    FormStatus::Redraw
                }
                status => status.into(),
            },
            AddIdentityKeyFormStep::Security(input) => match input.on_event(event) {
                InputStatus::Done(security_level) => {
                    self.security_result = Some(security_level);
                    self.step = AddIdentityKeyFormStep::KeyType(SelectInput::new(
                        KeyType::iter().collect(),
                    ));
                    FormStatus::Redraw
                }
                input => input.into(),
            },
            AddIdentityKeyFormStep::KeyType(input) => match input.on_event(event) {
                InputStatus::Done(key_type) => FormStatus::Done {
                    task: Task::Identity(IdentityTask::AddIdentityKey {
                        key_type,
                        security_level: self
                            .security_result
                            .expect("must be selected on previous steps"),
                        purpose: self
                            .purpose_result
                            .expect("must be selected on previous steps"),
                    }),
                    block: true,
                },
                input => input.into(),
            },
        }
    }

    fn form_name(&self) -> &'static str {
        "Add identity key"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        match &mut self.step {
            AddIdentityKeyFormStep::Purpose(input) => input.view(frame, area),
            AddIdentityKeyFormStep::Security(input) => input.view(frame, area),
            AddIdentityKeyFormStep::KeyType(input) => input.view(frame, area),
        }
    }

    fn step_name(&self) -> &'static str {
        match self.step {
            AddIdentityKeyFormStep::Purpose(_) => "Key purpose",
            AddIdentityKeyFormStep::Security(_) => "Key security level",
            AddIdentityKeyFormStep::KeyType(_) => "Key type",
        }
    }

    fn step_index(&self) -> u8 {
        match self.step {
            AddIdentityKeyFormStep::Purpose(_) => 0,
            AddIdentityKeyFormStep::Security(_) => 1,
            AddIdentityKeyFormStep::KeyType(_) => 2,
        }
    }

    fn steps_number(&self) -> u8 {
        3
    }
}
