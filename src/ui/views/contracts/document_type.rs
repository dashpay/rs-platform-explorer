//! UI defenitions for selected data contract.

mod broadcast_document;
mod broadcast_random_documents;

use dpp::{
    data_contract::{
        accessors::v0::DataContractV0Getters,
        document_type::{accessors::DocumentTypeV0Getters, DocumentType},
    },
    identifier::Identifier,
    identity::accessors::IdentityGettersV0,
    platform_value::string_encoding::Encoding,
    prelude::DataContract,
};
use futures::FutureExt;
use itertools::Itertools;
use tuirealm::{
    event::{Key, KeyEvent, KeyModifiers},
    tui::prelude::Rect,
    Frame,
};

use self::{
    broadcast_document::BroadcastSpecificDocumentForm,
    broadcast_random_documents::BroadcastRandomDocumentsCountForm,
};
use crate::{
    backend::{
        as_json_string, documents::DocumentTask, AppState, BackendEvent, CompletedTaskPayload, Task,
    },
    ui::{
        form::{
            parsers::DocumentQueryTextInputParser, FormController, FormStatus, Input, InputStatus,
            SelectInput, TextInput,
        },
        screen::{
            widgets::info::Info, ScreenCommandKey, ScreenController, ScreenFeedback,
            ScreenToggleKey,
        },
        views::documents::DocumentsQuerysetScreenController,
    },
    Event,
};

pub(super) struct SelectDocumentTypeFormController {
    input: SelectInput<String>,
    contract_name: String,
}

impl SelectDocumentTypeFormController {
    pub(super) fn new(contract_name: String, document_type_names: Vec<String>) -> Self {
        SelectDocumentTypeFormController {
            input: SelectInput::new(document_type_names),
            contract_name,
        }
    }
}

impl FormController for SelectDocumentTypeFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(document_type_name) => {
                let contract_name = self.contract_name.clone();
                let document_type_name = document_type_name.clone();

                FormStatus::NextScreen(Box::new(|app_state| {
                    async {
                        Box::new(
                            DocumentTypeScreenController::new(
                                app_state
                                    .loaded_identity
                                    .lock()
                                    .await
                                    .as_ref()
                                    .map(|identity| identity.id()),
                                contract_name,
                                document_type_name,
                                app_state,
                            )
                            .await,
                        ) as Box<dyn ScreenController>
                    }
                    .boxed()
                }))
            }
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Examine document type of the data contract"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Document type"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}

const LOADED_IDENTITY_COMMANDS: [ScreenCommandKey; 5] = [
    ScreenCommandKey::new("q", "Back to Contracts"),
    ScreenCommandKey::new("f", "Query"),
    ScreenCommandKey::new("o", "Query ours"),
    ScreenCommandKey::new("r", "Broadcast Random Documents"),
    ScreenCommandKey::new("b", "Broadcast Document"),
];

const NO_LOADED_IDENTITY_COMMANDS: [ScreenCommandKey; 4] = [
    ScreenCommandKey::new("q", "Back to Contracts"),
    ScreenCommandKey::new("f", "Query"),
    ScreenCommandKey::new("r", "Broadcast Random Documents"),
    ScreenCommandKey::new("b", "Broadcast Document"),
];

pub(super) struct DocumentTypeScreenController {
    identity_identifier: Option<Identifier>,
    data_contract: DataContract,
    data_contract_name: String,
    document_type: DocumentType,
    document_type_name: String,
    info: Info,
}

impl DocumentTypeScreenController {
    pub(super) async fn new(
        identity_identifier: Option<Identifier>,
        data_contract_name: String,
        document_type_name: String,
        app_state: &AppState,
    ) -> Self {
        let known_contracts_lock = app_state.known_contracts.lock().await;
        let data_contract = known_contracts_lock
            .get(&data_contract_name)
            .expect("expected a contract")
            .clone();
        let document_type = data_contract
            .document_type_for_name(&document_type_name)
            .expect("expected a document type")
            .to_owned_document_type();
        let document_type_properties_str =
            match serde_json::to_string_pretty(document_type.properties()) {
                Ok(string) => format!("========= Properties =========\n\n{}", string),
                Err(e) => {
                    tracing::error!(
                        "Error serializing to json string: {e} properties: {:?}",
                        document_type.properties()
                    );
                    as_json_string(document_type.properties())
                }
            };
        let document_type_indices_str = match serde_json::to_string_pretty(document_type.indices())
        {
            Ok(string) => format!("\n========= Indexes =========\n\n{}", string),
            Err(e) => {
                tracing::error!(
                    "Error serializing to json string: {e} indices: {:?}",
                    document_type.indices()
                );
                as_json_string(document_type.indices())
            }
        };
        let document_type_info = format!(
            "{}\n{}",
            document_type_properties_str, &document_type_indices_str
        );

        let info = Info::new_scrollable(&document_type_info);

        Self {
            identity_identifier,
            data_contract,
            data_contract_name,
            document_type,
            document_type_name,
            info,
        }
    }
}

impl ScreenController for DocumentTypeScreenController {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.info.view(frame, area)
    }

    fn name(&self) -> &'static str {
        "Document type"
    }

    fn command_keys(&self) -> &[ScreenCommandKey] {
        if self.identity_identifier.is_some() {
            &LOADED_IDENTITY_COMMANDS
        } else {
            &NO_LOADED_IDENTITY_COMMANDS
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
                code: Key::Char('f'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(QueryDocumentTypeFormController::new(
                self.data_contract.clone(),
                self.document_type.clone(),
                self.identity_identifier.clone(),
                false,
            ))),

            Event::Key(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(BroadcastRandomDocumentsCountForm::new(
                self.data_contract_name.clone(),
                self.document_type_name.clone(),
            ))),

            Event::Key(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(BroadcastSpecificDocumentForm::new(
                self.data_contract_name.clone(),
                self.document_type_name.clone(),
                self.document_type
                    .properties()
                    .keys()
                    .cloned()
                    .collect_vec(),
            ))),

            Event::Key(KeyEvent {
                code: Key::Char('o'),
                modifiers: KeyModifiers::NONE,
            }) => ScreenFeedback::Form(Box::new(QueryDocumentTypeFormController::new(
                self.data_contract.clone(),
                self.document_type.clone(),
                self.identity_identifier.clone(),
                true,
            ))),

            // Forward event to upper part of the screen for scrolls and stuff
            Event::Key(k) => {
                if self.info.on_event(k) {
                    ScreenFeedback::Redraw
                } else {
                    ScreenFeedback::None
                }
            }

            // Backend events handling
            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::QueryDocuments(_)),
                execution_result: Ok(CompletedTaskPayload::Documents(documents)),
            }) => {
                let data_contract = self.data_contract.clone();
                let document_type = self.document_type.clone();
                let identity_id = self.identity_identifier.clone();
                let documents = documents.clone();
                ScreenFeedback::NextScreen(Box::new(move |_| {
                    async move {
                        Box::new(DocumentsQuerysetScreenController::new(
                            data_contract,
                            document_type,
                            identity_id,
                            documents,
                        )) as Box<dyn ScreenController>
                    }
                    .boxed()
                }))
            }

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::QueryDocuments(_)),
                execution_result: Err(e),
            }) => {
                self.info = Info::new_error(&e);
                ScreenFeedback::Redraw
            }

            Event::Backend(
                BackendEvent::TaskCompleted {
                    task: Task::Document(DocumentTask::BroadcastRandomDocuments { .. }),
                    execution_result,
                }
                | BackendEvent::TaskCompletedStateChange {
                    task: Task::Document(DocumentTask::BroadcastRandomDocuments { .. }),
                    execution_result,
                    ..
                },
            ) => {
                self.info = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            _ => ScreenFeedback::None,
        }
    }
}

struct QueryDocumentTypeFormController {
    document_type: DocumentType,
    identity_id: Option<Identifier>,
    input: TextInput<DocumentQueryTextInputParser>,
}

impl QueryDocumentTypeFormController {
    fn new(
        data_contract: DataContract,
        document_type: DocumentType,
        identity_id: Option<Identifier>,
        ours_query: bool,
    ) -> Self {
        let ours_query_part = if ours_query && identity_id.is_some() {
            format!(
                "where `$ownerId` = '{}' ",
                identity_id.unwrap().to_string(Encoding::Base58)
            )
        } else {
            String::default()
        };
        let query = format!("Select * from {} {}", document_type.name(), ours_query_part);
        let parser = DocumentQueryTextInputParser::new(data_contract);
        Self {
            document_type,
            identity_id,
            input: TextInput::new_str_value_with_parser(parser, "Document Query", &query),
        }
    }
}

impl FormController for QueryDocumentTypeFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(query) => FormStatus::Done {
                task: Task::Document(DocumentTask::QueryDocuments(query)),
                block: true,
            },
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Get Documents by Query"
    }

    fn step_view(&mut self, frame: &mut Frame, area: Rect) {
        self.input.view(frame, area)
    }

    fn step_name(&self) -> &'static str {
        "Query"
    }

    fn step_index(&self) -> u8 {
        0
    }

    fn steps_number(&self) -> u8 {
        1
    }
}
