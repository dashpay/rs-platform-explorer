//! Application logic module, includes model and screen ids.

use std::time::Duration;

use tuirealm::{
    terminal::TerminalBridge,
    tui::prelude::{Constraint, Direction, Layout},
    Application, ApplicationError, AttrValue, Attribute, EventListenerCfg, NoUserEvent, Update,
};

use crate::components::*;

/// Screen identifiers
#[derive(Debug, Hash, Clone, Eq, PartialEq, Copy, strum::AsRefStr)]
pub(super) enum Screen {
    Main,
    Identity,
    GetIdentity,
}

/// Component identifiers, required to triggers screen switch which involves mounting and
/// unmounting.
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub(super) enum ComponentId {
    CommandPallet,
    Screen,
    Status,
    Breadcrumbs,
}

impl Default for Screen {
    fn default() -> Self {
        Screen::Main
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Message {
    AppClose,
    NextScreen(Screen),
    PrevScreen,
    ToggleFlag,
}

pub(super) struct Model {
    /// Application
    pub app: Application<ComponentId, Message, NoUserEvent>,
    /// Indicates that the application must quit
    pub quit: bool,
    /// Tells whether to redraw interface
    pub redraw: bool,
    /// Current screen
    pub current_screen: Screen,
    /// Breadcrumbs
    pub breadcrumbs: Vec<Screen>,
    /// Used to draw to terminal
    pub terminal: TerminalBridge,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            app: Self::init_app().expect("Unable to init the application"),
            quit: false,
            redraw: true,
            current_screen: Screen::Main,
            breadcrumbs: Vec::new(),
            terminal: TerminalBridge::new().expect("Cannot initialize terminal"),
        }
    }
}

impl Model {
    fn init_app() -> Result<Application<ComponentId, Message, NoUserEvent>, ApplicationError> {
        let mut app = Application::init(
            EventListenerCfg::default()
                .default_input_listener(Duration::from_millis(20))
                .poll_timeout(Duration::from_millis(10))
                .tick_interval(Duration::from_secs(1)),
        );

        // Mount components
        app.mount(ComponentId::Screen, Box::new(MainScreen::new()), Vec::new())?;
        app.mount(
            ComponentId::CommandPallet,
            Box::new(MainScreenCommands::new()),
            Vec::new(),
        )?;
        app.mount(
            ComponentId::Breadcrumbs,
            Box::new(Breadcrumbs::new()),
            Vec::new(),
        )?;
        app.mount(ComponentId::Status, Box::new(Status::new()), Vec::new())?;

        // Setting focus on the screen so it will react to events
        app.active(&ComponentId::CommandPallet)?;

        Ok(app)
    }

    pub fn view(&mut self) {
        self.terminal
            .raw_mut()
            .draw(|f| {
                // App layout: screen window, screen keys and status bar
                let outer_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [Constraint::Min(10), Constraint::Max(5), Constraint::Max(2)].as_ref(),
                    )
                    .split(f.size());

                // Status line layout
                let status_bar_layout = Layout::default()
                    .horizontal_margin(1)
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(20), Constraint::Max(20)].as_ref())
                    .split(outer_layout[2]);

                self.app.view(&ComponentId::Screen, f, outer_layout[0]);
                self.app
                    .view(&ComponentId::CommandPallet, f, outer_layout[1]);

                self.app
                    .view(&ComponentId::Breadcrumbs, f, status_bar_layout[0]);
                self.app.view(&ComponentId::Status, f, status_bar_layout[1]);
            })
            .expect("unable to render the application");
    }

    pub fn set_screen(&mut self, screen: Screen) {
        match screen {
            Screen::Main => {
                self.app
                    .remount(ComponentId::Screen, Box::new(MainScreen::new()), Vec::new())
                    .expect("unable to remount screen");
                self.app
                    .remount(
                        ComponentId::CommandPallet,
                        Box::new(MainScreenCommands::new()),
                        Vec::new(),
                    )
                    .expect("unable to remount screen");
            }
            Screen::Identity => {
                self.app
                    .remount(
                        ComponentId::Screen,
                        Box::new(IdentityScreen::new()),
                        Vec::new(),
                    )
                    .expect("unable to remount screen");
                self.app
                    .remount(
                        ComponentId::CommandPallet,
                        Box::new(IdentityScreenCommands::new()),
                        Vec::new(),
                    )
                    .expect("unable to remount screen");
            }
            Screen::GetIdentity => {
                self.app
                    .remount(
                        ComponentId::Screen,
                        Box::new(GetIdentityScreen::new()),
                        Vec::new(),
                    )
                    .expect("unable to remount screen");
                self.app
                    .remount(
                        ComponentId::CommandPallet,
                        Box::new(GetIdentityScreenCommands::new()),
                        Vec::new(),
                    )
                    .expect("unable to remount screen");
            }
        }
        self.app
            .attr(
                &ComponentId::Breadcrumbs,
                Attribute::Text,
                AttrValue::String(
                    self.breadcrumbs
                        .iter()
                        .chain(std::iter::once(&self.current_screen))
                        .map(AsRef::as_ref)
                        .fold(String::new(), |mut acc, segment| {
                            acc.push_str(segment);
                            acc.push_str(" / ");
                            acc
                        }),
                ),
            )
            .expect("cannot set breadcrumbs");
    }
}

impl Update<Message> for Model {
    fn update(&mut self, message: Option<Message>) -> Option<Message> {
        if let Some(message) = message {
            // Set redraw
            self.redraw = true;
            // Match message
            match message {
                Message::AppClose => {
                    self.quit = true; // Terminate
                    None
                }
                Message::NextScreen(s) => {
                    self.breadcrumbs.push(self.current_screen);
                    self.current_screen = s;
                    self.set_screen(s);
                    None
                }
                Message::PrevScreen => {
                    let screen = self
                        .breadcrumbs
                        .pop()
                        .expect("must not be triggered on the main screen");
                    self.current_screen = screen;
                    self.set_screen(screen);
                    None
                }
                // Just redraw
                Message::ToggleFlag => None,
            }
        } else {
            None
        }
    }
}
