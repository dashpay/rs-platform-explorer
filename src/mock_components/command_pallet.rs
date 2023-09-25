//! Command Pallet is a "mock component" that helps to quickly setup keystroke navigation
//! for each screen including execution of actions with togglable flags.

use std::collections::HashMap;

use tui_realm_stdlib::Table;
use tuirealm::{
    command::{Cmd, CmdResult},
    props::TextSpan,
    tui::prelude::Rect,
    AttrValue, Attribute, Frame, MockComponent, State, StateValue,
};

const KEYS_PER_ROW: usize = 3;

#[derive(PartialEq)]
pub(crate) enum KeyType {
    Toggle,
    Command,
}

pub(crate) struct CommandPalletKey {
    pub key: char,
    pub description: &'static str,
    pub key_type: KeyType,
}

pub(crate) struct CommandPallet {
    keys: Vec<CommandPalletKey>,
    state: State,
}

impl CommandPallet {
    pub(crate) fn new(keys: Vec<CommandPalletKey>) -> Self {
        let state_map = keys
            .iter()
            .filter(|k| k.key_type == KeyType::Toggle)
            .map(|k| (k.key.to_string(), StateValue::Bool(false)))
            .collect();

        CommandPallet {
            state: State::Map(state_map),
            keys,
        }
    }
}

impl MockComponent for CommandPallet {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let mut table_vec = Vec::new();
        let toggles = match &self.state {
            State::Map(hm) => hm,
            _ => unreachable!("State for `CommandPallet` is always a map"),
        };

        for row in self.keys.chunks(KEYS_PER_ROW) {
            let mut row_vec = Vec::new();
            for key in row {
                let mut span = TextSpan::new(format!("{} - {}", key.key, key.description));

                if matches!(key.key_type, KeyType::Toggle) {
                    span = span.italic();
                }

                span = if matches!(
                    toggles.get(key.key.to_string().as_str()),
                    Some(StateValue::Bool(true))
                ) {
                    span.bold()
                } else {
                    span
                };
                row_vec.push(span);
            }

            table_vec.push(row_vec);
        }

        Table::default().table(table_vec).view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        None
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {}

    fn state(&self) -> State {
        self.state.clone()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        let toggles = match &mut self.state {
            State::Map(hm) => hm,
            _ => unreachable!("State for `CommandPallet` is always a map"),
        };
        match cmd {
            Cmd::Type(c) => {
                if let Some(StateValue::Bool(flag)) = toggles.get_mut(c.to_string().as_str()) {
                    *flag = !*flag;
                    CmdResult::Changed(self.state.clone())
                } else {
                    CmdResult::None
                }
            }
            _ => CmdResult::None,
        }
    }
}
