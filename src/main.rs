use std::{fs::File, panic, time::Duration};

use crossterm::event::{Event as TuiEvent, EventStream};
use dash_sdk::{RequestSettings, SdkBuilder};
use dpp::{identity::accessors::IdentityGettersV0, version::PlatformVersion};
use futures::{future::OptionFuture, select, FutureExt, StreamExt};
use rs_platform_explorer::{
    backend::{insight::InsightAPIClient, Backend},
    config::Config,
    ui::{IdentityBalance, Ui, UiFeedback},
    Event,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Initialize logger
    let log_file = File::create("explorer.log").expect("create log file");

    let filter = EnvFilter::try_new("info")
        .unwrap()
        .add_directive("rs_dapi_client=off".parse().unwrap()); // Turn off all logs from `rs-dapi-client`

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(log_file)
        .with_ansi(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("Unable to set global default subscriber");

    // Test log statement
    tracing::info!("Logger initialized successfully");

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

    let backend = Backend::new(&sdk, insight, config).await;

    // Add loaded identity to known identities if it's not already there
    // And set selected_strategy to None
    {
        let state = backend.state();
        let loaded_identity = state.loaded_identity.lock().await;
        let mut selected_strategy = state.selected_strategy.lock().await;
        let mut known_identities = state.known_identities.lock().await;

        if let Some(loaded_identity) = loaded_identity.as_ref() {
            known_identities
                .entry(loaded_identity.id())
                .or_insert_with(|| loaded_identity.clone());
        }

        *selected_strategy = None;
    }

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
