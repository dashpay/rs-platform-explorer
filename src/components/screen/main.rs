//! Main screen module, also known as a welcome screen.

use tui_realm_stdlib::{Paragraph, Table};
use tuirealm::{
    command::{Cmd, CmdResult},
    event::{Key, KeyEvent, KeyModifiers},
    props::TextSpan,
    Component, Event, MockComponent, NoUserEvent,
};

use crate::{
    app::{Message, Screen},
    mock_components::{CommandPallet, CommandPalletKey, KeyType},
};

#[derive(MockComponent)]
pub(crate) struct MainScreen {
    component: Paragraph,
}

impl MainScreen {
    pub(crate) fn new() -> Self {
        MainScreen {
            component: Paragraph::default().text(
                [
                    TextSpan::new("Welcome to Platform TUI!"),
                    TextSpan::new(""),
                    TextSpan::new("Use keys listed in the section below to switch screens and execute commands."),
                    TextSpan::new("Some of them require signature and are disabled until an identity key is loaded."),
                    TextSpan::new(""),
                    TextSpan::new("Italics are used to mark flags.").italic(),
                    TextSpan::new("Bold italics are flags that are enabled.").italic().bold(),
                ]
                .as_ref(),
            ),
        }
    }
}

impl Component<Message, NoUserEvent> for MainScreen {
    fn on(&mut self, _ev: Event<NoUserEvent>) -> Option<Message> {
        None
    }
}

#[derive(MockComponent)]
pub(crate) struct MainScreenCommands {
    component: CommandPallet,
}

impl MainScreenCommands {
    pub(crate) fn new() -> Self {
        MainScreenCommands {
            component: CommandPallet::new(vec![
                CommandPalletKey {
                    key: 'q',
                    description: "Quit",
                    key_type: KeyType::Command,
                },
                CommandPalletKey {
                    key: 'i',
                    description: "Identities",
                    key_type: KeyType::Command,
                },
            ]),
        }
    }
}

impl Component<Message, NoUserEvent> for MainScreenCommands {
    fn on(&mut self, ev: Event<NoUserEvent>) -> Option<Message> {
        match ev {
            Event::Keyboard(KeyEvent {
                code: Key::Char('q'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Message::AppClose),
            Event::Keyboard(KeyEvent {
                code: Key::Char('i'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Message::NextScreen(Screen::Identity)),
            _ => None,
        }
    }
}
