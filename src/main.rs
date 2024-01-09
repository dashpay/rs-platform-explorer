mod backend;
mod config;
mod ui;

use std::{fs::File, panic, time::Duration};

use crossterm::event::{Event as TuiEvent, EventStream};
use dash_platform_sdk::{RequestSettings, SdkBuilder};
use dpp::{identity::accessors::IdentityGettersV0, version::PlatformVersion};
use futures::{future::OptionFuture, select, FutureExt, StreamExt};
use tracing_subscriber::EnvFilter;
use tuirealm::event::KeyEvent;
use ui::IdentityBalance;

use self::{
    backend::{Backend, BackendEvent, Task},
    ui::{Ui, UiFeedback},
};
use crate::{backend::insight::InsightAPIClient, config::Config};

pub(crate) enum Event<'s> {
    Key(KeyEvent),
    Backend(BackendEvent<'s>),
    RedrawDebounceTimeout,
}

#[tokio::main]
async fn main() {
    // Initialize logger
    let log_file = File::create("explorer.log").expect("create log file");

    let subscriber = tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(log_file)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("can't initialize logging");

    // Log panics
    let default_panic_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        let message = panic_info
            .payload()
            .downcast_ref::<&str>()
            .unwrap_or(&"unknown");

        let location = panic_info
            .location()
            .unwrap_or_else(|| panic::Location::caller());

        tracing::error!(
            location = tracing::field::display(location),
            "Panic occurred: {}",
            message
        );

        default_panic_hook(panic_info);
    }));

    // Load configuration
    let config = Config::load();

    // Setup Platform SDK
    let address_list = config.dapi_address_list();
    let request_settings = RequestSettings {
        connect_timeout: Some(Duration::from_secs(10)),
        timeout: Some(Duration::from_secs(10)),
        retries: None,
        ban_failed_address: Some(false),
    };
    let sdk = SdkBuilder::new(address_list)
        .with_version(PlatformVersion::get(1).unwrap())
        .with_core(
            &config.core_host,
            config.core_rpc_port,
            &config.core_rpc_user,
            &config.core_rpc_password,
        )
        .with_settings(request_settings)
        .build()
        .expect("expected to build sdk");

    let insight = InsightAPIClient::new(config.insight_api_uri());

    let backend = Backend::new(sdk.as_ref(), insight, config).await;

    let initial_identity_balance = backend
        .state()
        .loaded_identity
        .lock()
        .await
        .as_ref()
        .map(|identity| IdentityBalance::from_credits(identity.balance()));

    let mut ui = Ui::new(initial_identity_balance);

    let mut active = true;

    let mut terminal_event_stream = EventStream::new().fuse();
    let mut backend_task: OptionFuture<_> = None.into();
    let mut ui_debounced_redraw: OptionFuture<_> = None.into();

    while active {
        let event = select! {
            terminal_event = terminal_event_stream.next() => match terminal_event {
                None => panic!("terminal event stream closed unexpectedly"),
                Some(Err(_)) => panic!("terminal event stream closed unexpectedly"),
                Some(Ok(TuiEvent::Resize(_, _))) => {ui.redraw(); continue },
                Some(Ok(TuiEvent::Key(key_event))) => Some(Event::Key(key_event.into())),
                _ => None
            },
            backend_task_finished = backend_task => backend_task_finished.map(Event::Backend),
            ui_redraw = ui_debounced_redraw => ui_redraw.map(|_| Event::RedrawDebounceTimeout),
        };

        let ui_feedback = match event {
            Some(event @ (Event::Backend(_) | Event::Key(_))) => {
                ui.on_event(backend.state(), event).await
            }
            Some(Event::RedrawDebounceTimeout) => {
                ui.redraw();
                UiFeedback::None
            }
            _ => UiFeedback::None,
        };

        match ui_feedback {
            UiFeedback::Quit => active = false,
            UiFeedback::ExecuteTask(task) => {
                backend_task = Some(backend.run_task(task.clone()).boxed_local().fuse()).into();
                ui.redraw();
            }
            UiFeedback::Redraw => {
                ui_debounced_redraw = Some(
                    tokio::time::sleep(Duration::from_millis(10))
                        .boxed_local()
                        .fuse(),
                )
                .into();
            }
            UiFeedback::None => (),
        }
    }
}
