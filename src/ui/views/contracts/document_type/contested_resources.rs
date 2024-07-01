//! Contested resources screen

use dpp::{
    data_contract::{
        accessors::v0::DataContractV0Getters,
        document_type::{accessors::DocumentTypeV0Getters, DocumentType},
        DataContract,
    },
    identifier::Identifier,
    platform_value::{string_encoding::Encoding, Value},
    voting::{
        vote_choices::resource_vote_choice::ResourceVoteChoice,
        vote_polls::{
            contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll, VotePoll,
        },
    },
};
use drive_proof_verifier::types::{Contenders, ContestedResource, ContestedResources};
use tuirealm::{event::KeyEvent, tui::prelude::Rect, Frame};

use crate::{
    backend::{documents::DocumentTask, Task},
    ui::form::{FormController, FormStatus, Input, InputStatus, SelectInput},
};

use tuirealm::{
    command::{self, Cmd},
    event::{Key, KeyModifiers},
    props::{BorderSides, Borders, Color, TextSpan},
    tui::prelude::{Constraint, Direction, Layout},
    AttrValue, Attribute, MockComponent,
};

use crate::{
    backend::{as_json_string, BackendEvent, CompletedTaskPayload},
    ui::screen::{
        widgets::info::Info, ScreenCommandKey, ScreenController, ScreenFeedback, ScreenToggleKey,
    },
    Event,
};

const COMMAND_KEYS: [ScreenCommandKey; 4] = [
    ScreenCommandKey::new("q", "Back"),
    ScreenCommandKey::new("↓", "Next resource"),
    ScreenCommandKey::new("↑", "Prev resource"),
    ScreenCommandKey::new("v", "Vote"),
];

pub(crate) struct ContestedResourcesScreenController {
    current_batch: ContestedResources,
    resource_select: tui_realm_stdlib::List,
    resource_view: Info,
    data_contract: DataContract,
    document_type: DocumentType,
}

impl ContestedResourcesScreenController {
    pub(crate) fn new(
        current_batch: ContestedResources,
        data_contract: DataContract,
        document_type: DocumentType,
    ) -> Self {
        let mut resource_select = tui_realm_stdlib::List::default()
            .rows(
                current_batch
                    .0
                    .iter()
                    .enumerate()
                    .map(|(_, v)| {
                        vec![TextSpan::new(match v {
                            ContestedResource::Value(value) => value.to_string(),
                        })]
                    })
                    .collect(),
            )
            .borders(
                Borders::default()
                    .sides(BorderSides::LEFT | BorderSides::TOP | BorderSides::BOTTOM),
            )
            .selected_line(0)
            .highlighted_color(Color::Magenta);
        resource_select.attr(Attribute::Scroll, AttrValue::Flag(true));
        resource_select.attr(Attribute::Focus, AttrValue::Flag(true));

        let resource_view = Info::new_scrollable(
            &current_batch
                .0
                .get(0)
                .and_then(|v| match v {
                    ContestedResource::Value(value) => Some(as_json_string(value)),
                })
                .unwrap_or_else(String::new),
        );

        Self {
            current_batch,
            resource_select,
            resource_view,
            data_contract,
            document_type,
        }
    }

    fn update_resource_view(&mut self) {
        self.resource_view = Info::new_scrollable(
            &self
                .current_batch
                .0
                .get(self.resource_select.state().unwrap_one().unwrap_usize())
                .and_then(|v| match v {
                    ContestedResource::Value(value) => Some(as_json_string(value)),
                })
                .unwrap_or_else(String::new),
        );
    }

    fn get_selected_resource(&self) -> Option<&Value> {
        let state = self.resource_select.state();
        let selected_index = state.unwrap_one().unwrap_usize();
        self.current_batch
            .0
            .get(selected_index)
            .and_then(|doc| match doc {
                ContestedResource::Value(value) => Some(value),
            })
    }
}

impl ScreenController for ContestedResourcesScreenController {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Max(40), Constraint::Min(1)].as_ref())
            .split(area);

        self.resource_select.view(frame, layout[0]);
        self.resource_view.view(frame, layout[1]);
    }

    fn name(&self) -> &'static str {
        "Contested resources"
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
                code: Key::Char('v'),
                modifiers: KeyModifiers::NONE,
            }) => {
                let resource = self
                    .get_selected_resource()
                    .expect("Expected to get a resource from the selection");

                let index_values = vec![Value::from("dash"), resource.clone()]; // hardcoded for dpns

                let index = self
                    .document_type
                    .find_contested_index()
                    .expect("Expected to find a contested index");

                let index_name = &index.name;

                ScreenFeedback::Task {
                    task: Task::Document(DocumentTask::QueryVoteContenders(
                        index_name.to_string(),
                        index_values,
                        self.document_type.name().to_string(),
                        self.data_contract.id(),
                    )),
                    block: true,
                }
            }

            // Resource selection keys
            Event::Key(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.resource_select
                    .perform(Cmd::Move(command::Direction::Down));
                self.update_resource_view();
                ScreenFeedback::Redraw
            }
            Event::Key(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.resource_select
                    .perform(Cmd::Move(command::Direction::Up));
                self.update_resource_view();
                ScreenFeedback::Redraw
            }

            // Backend events handling
            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::QueryVoteContenders(_, _, _, _)),
                execution_result,
            }) => match execution_result {
                Ok(CompletedTaskPayload::ContestedResourceContenders(vote_poll, contenders)) => {
                    ScreenFeedback::Form(Box::new(ContestedResourceVoteFormController::new(
                        vote_poll.clone(),
                        contenders.clone(),
                    )) as Box<dyn FormController>)
                }
                Err(_) => {
                    self.resource_view = Info::new_from_result(execution_result);
                    ScreenFeedback::Redraw
                }
                _ => ScreenFeedback::None,
            },

            Event::Backend(BackendEvent::TaskCompleted {
                task: Task::Document(DocumentTask::VoteOnContestedResource(_, _)),
                execution_result,
            }) => {
                self.resource_view = Info::new_from_result(execution_result);
                ScreenFeedback::Redraw
            }

            _ => ScreenFeedback::None,
        }
    }
}

pub struct ContestedResourceVoteFormController {
    input: SelectInput<String>,
    vote_poll: ContestedDocumentResourceVotePoll,
}

impl ContestedResourceVoteFormController {
    pub fn new(vote_poll: ContestedDocumentResourceVotePoll, contenders: Contenders) -> Self {
        let mut options: Vec<String> = vec![
            format!(
                "Abstain ({})",
                contenders.abstain_vote_tally.unwrap_or_default()
            ),
            format!("Lock ({})", contenders.lock_vote_tally.unwrap_or_default()),
        ];
        for contender in contenders.contenders {
            let identity_id = contender.0;
            options.push(format!(
                "{} ({})",
                identity_id.to_string(Encoding::Base58),
                contender.1.vote_tally().unwrap_or_default()
            ));
        }
        Self {
            input: SelectInput::new(options),
            vote_poll,
        }
    }
}

impl FormController for ContestedResourceVoteFormController {
    fn on_event(&mut self, event: KeyEvent) -> FormStatus {
        match self.input.on_event(event) {
            InputStatus::Done(vote_string) => {
                let parsed_vote_string = vote_string.split_whitespace().next().unwrap_or("");
                let vote = match parsed_vote_string {
                    "Abstain" => ResourceVoteChoice::Abstain,
                    "Lock" => ResourceVoteChoice::Lock,
                    _ => ResourceVoteChoice::TowardsIdentity(
                        Identifier::from_string(&parsed_vote_string, Encoding::Base58)
                            .expect("Expected to convert String to Identifier"),
                    ),
                };
                FormStatus::Done {
                    task: Task::Document(DocumentTask::VoteOnContestedResource(
                        VotePoll::ContestedDocumentResourceVotePoll(self.vote_poll.clone()),
                        vote,
                    )),
                    block: true,
                }
            }
            InputStatus::Exit => FormStatus::Exit,
            status => status.into(),
        }
    }

    fn form_name(&self) -> &'static str {
        "Vote on contested resource"
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
