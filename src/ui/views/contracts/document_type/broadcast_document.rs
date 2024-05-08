//! Form to broadcast a document with specified properties.

use std::collections::HashMap;
use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{documents::DocumentTask, Task},
    ui::form::{
        parsers::DefaultTextInputParser, FormController, FormStatus, Input, InputStatus, TextInput,
    },
};

pub(super) struct BroadcastSpecificDocumentForm {
    inputs: HashMap<String, TextInput<DefaultTextInputParser<String>>>, // Corrected generic type
    values: HashMap<String, String>, // To store the values entered
    data_contract_name: String,
    document_type_name: String,
    current_step: usize,
}

impl BroadcastSpecificDocumentForm {
    pub fn new(
        data_contract_name: String,
        document_type_name: String,
        properties: Vec<String>,
    ) -> Self {
        let mut inputs = HashMap::new();
        for property in properties {
            inputs.insert(property.clone(), TextInput::new("Enter property"));
        }

        Self {
            inputs,
            values: HashMap::new(),
            data_contract_name,
            document_type_name,
            current_step: 0,
        }
    }
}

impl FormController for BroadcastSpecificDocumentForm {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        let property_names: Vec<String> = self.inputs.keys().cloned().collect();
        let current_property = &property_names[self.current_step];

        if let Some(input) = self.inputs.get_mut(current_property) {
            match input.on_event(event) {
                InputStatus::Done(value) => {
                    // Store the value entered
                    self.values.insert(current_property.clone(), value);

                    // Move to the next step or complete the form
                    if self.current_step + 1 < self.inputs.len() {
                        self.current_step += 1;
                        FormStatus::None
                    } else {
                        FormStatus::Done {
                            task: Task::Document(DocumentTask::BroadcastDocument {
                                data_contract_name: self.data_contract_name.clone(),
                                document_type_name: self.document_type_name.clone(),
                                properties: self.values.clone(),
                            }),
                            block: true,
                        }
                    }
                }
                _ => FormStatus::Exit,
            }
        } else {
            FormStatus::Exit
        }
    }

    fn form_name(&self) -> &'static str {
        "Broadcast specific document"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        let property_names: Vec<String> = self.inputs.keys().cloned().collect();
        if let Some(input) = self.inputs.get_mut(&property_names[self.current_step]) {
            input.view(frame, area);
        }
    }

    fn step_name(&self) -> &'static str {
        "Step name"
    }

    fn step_index(&self) -> u8 {
        self.current_step as u8
    }

    fn steps_number(&self) -> u8 {
        self.inputs.len() as u8
    }
}
