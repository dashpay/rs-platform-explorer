//! View for fetched documents navigation and inspection.

use std::collections::BTreeMap;

use dpp::{
    data_contract::{document_type::DocumentType, DataContract},
    document::{Document, DocumentV0Getters},
    fee::Credits,
    platform_value::{btreemap_extensions::BTreeValueMapHelper, string_encoding::Encoding},
    prelude::Identifier,
};
use tuirealm::{
    command::{self, Cmd},
    event::{Key, KeyEvent, KeyModifiers},
    props::{BorderSides, Borders, Color, TextSpan},
    tui::prelude::{Constraint, Direction, Layout, Rect},
    AttrValue, Attribute, Frame, MockComponent,
};

use crate::{
    backend::{as_toml, documents::DocumentTask, BackendEvent, Task},
    ui::{
        form::{
            parsers::DefaultTextInputParser, FormController, FormStatus, Input, InputStatus,
            SelectInput, TextInput,
        },
        screen::{
            widgets::info::Info, ScreenCommandKey, ScreenController, ScreenFeedback,
            ScreenToggleKey,
        },
    },
    Event,
};

const BASE_COMMAND_KEYS: [ScreenCommandKey; 5] = [
    ScreenCommandKey::new("q", "Back to Contracts"),
    ScreenCommandKey::new("C-n", "Next document"),
    ScreenCommandKey::new("C-p", "Prev document"),
    ScreenCommandKey::new("↓", "Scroll doc down"),
    ScreenCommandKey::new("↑", "Scroll doc up"),
];

const PURCHASE_COMMAND_KEYS: [ScreenCommandKey; 6] = [
    ScreenCommandKey::new("q", "Back to Contracts"),
    ScreenCommandKey::new("C-n", "Next document"),
    ScreenCommandKey::new("C-p", "Prev document"),
    ScreenCommandKey::new("↓", "Scroll doc down"),
    ScreenCommandKey::new("↑", "Scroll doc up"),
    ScreenCommandKey::new("p", "Purchase"),
];

const DOCUMENT_OWNED_COMMAND_KEYS: [ScreenCommandKey; 7] = [
    ScreenCommandKey::new("q", "Back to Contracts"),
    ScreenCommandKey::new("C-n", "Next document"),
    ScreenCommandKey::new("C-p", "Prev document"),
    ScreenCommandKey::new("↓", "Scroll doc down"),
    ScreenCommandKey::new("↑", "Scroll doc up"),
    ScreenCommandKey::new("s", "Set price"),
    ScreenCommandKey::new("t", "Transfer"),
];

pub(crate) struct DocumentsQuerysetScreenController {
    data_contract: DataContract,
    document_type: DocumentType,
    identity_id: Option<Identifier>,
    current_batch: Vec<Option<Document>>,
    document_select: tui_realm_stdlib::List,
    document_view: Info,
}

impl DocumentsQuerysetScreenController {
    pub(crate) fn new(
        data_contract: DataContract,
        document_type: DocumentType,
        identity_id: Option<Identifier>,
        current_batch: BTreeMap<Identifier, Option<Document>>,
    ) -> Self {
        let mut document_select = tui_realm_stdlib::List::default()
            .rows(
                current_batch
                    .keys()
                    .map(|v| vec![TextSpan::new(v.to_string(Encoding::Base58))])
                    .collect(),
            )
            .borders(
                Borders::default()
                    .sides(BorderSides::LEFT | BorderSides::TOP | BorderSides::BOTTOM),
            )
            .selected_line(0)
            .highlighted_color(Color::Magenta);
        document_select.attr(Attribute::Scroll, AttrValue::Flag(true));
        document_select.attr(Attribute::Focus, AttrValue::Flag(true));

        let document_view = Info::new_scrollable(
            &current_batch
                .first_key_value()
                .map(|(_, v)| as_toml(v))
                .unwrap_or_else(String::new),
        );

        Self {
            data_contract,
            document_type,
            identity_id,
            current_batch: current_batch.into_values().collect(),
            document_select,
            document_view,
        }
    }

    fn update_document_view(&mut self) {
        self.document_view = Info::new_scrollable(
            &self
                .current_batch
                .get(self.document_select.state().unwrap_one().unwrap_usize())
                .map(|v| as_toml(&v))
                .unwrap_or_else(String::new),
        );
    }

    pub(crate) fn document_is_purchasable(&self, idx: usize) -> bool {
        if let Some(Some(doc)) = self.current_batch.get(idx) {
            match doc.properties().get_optional_integer::<Credits>("$price") {
                Ok(price) => {
                    if let Some(id) = self.identity_id {
                        if doc.owner_id() == id {
                            false
                        } else {
                            price.is_some()
                        }
                    } else {
                        price.is_some()
                    }
                }
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub(crate) fn document_is_ours(&self, idx: usize) -> bool {
        if let Some(Some(doc)) = self.current_batch.get(idx) {
            if let Some(id) = self.identity_id {
                doc.owner_id() == id
            } else {
                false
            }
        } else {
            false
        }
    }
}

impl ScreenController for DocumentsQuerysetScreenController {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Max(40), Constraint::Min(1)].as_ref())
            .split(area);

        self.document_select.view(frame, layout[0]);
        self.document_view.view(frame, layout[1]);
    }

    fn name(&self) -> &'static str {
        "Documents queryset"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        // Only display purchase option if the document is purchasable
        let idx = self.document_select.state().unwrap_one().unwrap_usize();
        let purchasable = self.document_is_purchasable(idx);
        if purchasable {
            PURCHASE_COMMAND_KEYS.as_ref()
        } else if self.document_is_ours(idx) {
            DOCUMENT_OWNED_COMMAND_KEYS.as_ref()
        } else {
            BASE_COMMAND_KEYS.as_ref()
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
                code: Key::Char('p'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let idx = self.document_select.state().unwrap_one().unwrap_usize();
                if self.document_is_purchasable(idx) {
                    if let Some(Some(doc)) = self.current_batch.get(idx) {
                        ScreenFeedback::Form(Box::new(ConfirmDocumentPurchaseFormController::new(
                            self.data_contract.clone(),
                            self.document_type.clone(),
                            doc.clone(),
                        )))
                    } else {
                        panic!("Selected document didn't exist?")
                    }
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('s'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let idx = self.document_select.state().unwrap_one().unwrap_usize();
                if self.document_is_ours(idx) {
                    if let Some(Some(doc)) = self.current_batch.get(idx) {
                        ScreenFeedback::Form(Box::new(SetDocumentPriceFormController::new(
                            self.data_contract.clone(),
                            self.document_type.clone(),
                            doc.clone(),
                        )))
                    } else {
                        panic!("Selected document didn't exist?")
                    }
                } else {
                    ScreenFeedback::None
                }
            }
            Event::Key(KeyEvent {
                code: Key::Char('t'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let idx = self.document_select.state().unwrap_one().unwrap_usize();
                if self.document_is_ours(idx) {
                    if let Some(Some(doc)) = self.current_batch.get(idx) {
                        ScreenFeedback::Form(Box::new(TransferDocumentFormController::new(
                            self.data_contract.clone(),
                            self.document_type.clone(),
                            doc.clone(),
                        )))
                    } else {
                        panic!("Selected document didn't exist?")
                    }
                } else {
                    ScreenFeedback::None
                }
            }

            // Document view keys
            Event::Key(
                key_event @ KeyEvent {
                    code: Key::Down | Key::Up,
                    modifiers: KeyModifiers::NONE,
                },
            ) => {
                self.document_view.on_event(key_event);
                ScreenFeedback::Redraw
            }

            // Document selection keys
            Event::Key(KeyEvent {
                code: Key::Char('n'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.document_select
                    .perform(Cmd::Move(command::Direction::Down));
                self.update_document_view();
                ScreenFeedback::Redraw
            }
            Event::Key(KeyEvent {
                code: Key::Char('p'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.document_select
                    .perform(Cmd::Move(command::Direction::Up));
                self.update_document_view();
                ScreenFeedback::Redraw
            }

            // Backend events handling
            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::PurchaseDocument { .. }),
                execution_result,
            }) => {
                self.document_view = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }
            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::SetDocumentPrice { .. }),
                execution_result,
            }) => {
                self.document_view = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }
            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::TransferDocument { .. }),
                execution_result,
            }) => {
                self.document_view = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            _ => ScreenFeedback::None,
        }
    }
}

pub struct ConfirmDocumentPurchaseFormController {
    confirm_input: SelectInput<String>,
    data_contract: DataContract,
    document_type: DocumentType,
    document: Document,
}

impl ConfirmDocumentPurchaseFormController {
    pub fn new(
        data_contract: DataContract,
        document_type: DocumentType,
        document: Document,
    ) -> Self {
        Self {
            confirm_input: SelectInput::new(vec!["Yes".to_string(), "No".to_string()]),
            data_contract,
            document_type,
            document,
        }
    }
}

impl FormController for ConfirmDocumentPurchaseFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.confirm_input.on_event(event) {
            InputStatus::Done(choice) => {
                if choice == "Yes" {
                    FormStatus::Done {
                        task: Task::Document(DocumentTask::PurchaseDocument {
                            data_contract: self.data_contract.clone(),
                            document_type: self.document_type.clone(),
                            document: self.document.clone(),
                        }),
                        block: true,
                    }
                } else {
                    FormStatus::Exit
                }
            }
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Purchase document confirmation"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.confirm_input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Confirm"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

pub struct SetDocumentPriceFormController {
    input: TextInput<DefaultTextInputParser<f64>>,
    data_contract: DataContract,
    document_type: DocumentType,
    document: Document,
}

impl SetDocumentPriceFormController {
    pub fn new(
        data_contract: DataContract,
        document_type: DocumentType,
        document: Document,
    ) -> Self {
        Self {
            input: TextInput::new("Amount (in Dash)"),
            data_contract,
            document_type,
            document,
        }
    }
}

impl FormController for SetDocumentPriceFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(amount) => FormStatus::Done {
                task: Task::Document(DocumentTask::SetDocumentPrice {
                    amount: (amount * 100_000_000_000.0) as u64,
                    data_contract: self.data_contract.clone(),
                    document_type: self.document_type.clone(),
                    document: self.document.clone(),
                }),
                block: true,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Set document price"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Amount"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

pub struct TransferDocumentFormController {
    input: TextInput<DefaultTextInputParser<String>>,
    data_contract: DataContract,
    document_type: DocumentType,
    document: Document,
}

impl TransferDocumentFormController {
    pub fn new(
        data_contract: DataContract,
        document_type: DocumentType,
        document: Document,
    ) -> Self {
        Self {
            input: TextInput::new("Base58 ID"),
            data_contract,
            document_type,
            document,
        }
    }
}

impl FormController for TransferDocumentFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(recipient_address) => FormStatus::Done {
                task: Task::Document(DocumentTask::TransferDocument {
                    recipient_address,
                    data_contract: self.data_contract.clone(),
                    document_type: self.document_type.clone(),
                    document: self.document.clone(),
                }),
                block: true,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Transfer document"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Recipient address"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}
