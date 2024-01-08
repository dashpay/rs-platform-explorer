use std::{panic, sync::Arc, time::Duration};

use clap::Parser;
use dash_platform_sdk::{
    platform::{Fetch, Identifier},
    SdkBuilder,
};
use dpp::{
    data_contract::{accessors::v0::DataContractV0Getters, DataContract},
    data_contracts::dpns_contract,
    identity::accessors::IdentityGettersV0,
    version::PlatformVersion,
};
use rs_dapi_client::RequestSettings;
use rs_platform_explorer::{
    backend::{
        identities::IdentityTask, insight::InsightAPIClient, wallet::WalletTask, Backend, Task,
    },
    config::Config,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
struct Args {
    #[arg(
        short,
        long,
        help = "The number of connections to open to each endpoint simultaneously"
    )]
    connections: u16,
    #[arg(
        short,
        long,
        help = "The duration (in seconds) for which to handle the load test"
    )]
    time: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize logger
    tracing_subscriber::fmt::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

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
            %location,
            "Panic occurred: {}",
            message
        );

        default_panic_hook(panic_info);
    }));

    // Load configuration
    let config = Config::load();

    // Setup Platform SDK
    let address_list = config.dapi_address_list();

    // Configure SDK for high throughput
    let request_settings = RequestSettings {
        connect_timeout: Some(Duration::from_secs(60)),
        timeout: None,
        retries: None,
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

    let backend = Backend::new(sdk, insight, config.clone()).await;

    // Create wallet if not initialized
    if backend.state().loaded_wallet.lock().await.is_none() {
        let Some(private_key) = config.wallet_private_key else {
            panic!("Wallet not initialized and no private key provided");
        };

        tracing::info!("Wallet not initialized, creating new wallet with configured private key");

        backend
            .run_task(Task::Wallet(WalletTask::AddByPrivateKey(
                private_key.clone(),
            )))
            .await;
    }

    // Refresh wallet balance
    backend.run_task(Task::Wallet(WalletTask::Refresh)).await;

    let balance = backend
        .state()
        .loaded_wallet
        .lock()
        .await
        .as_ref()
        .unwrap()
        .balance();

    tracing::info!("Wallet is initialized with {} Dash", balance / 100000000);

    // Register identity if there is no yet
    if backend.state().loaded_identity.lock().await.is_none() {
        let dash = 15;
        let amount = dash * 100000000; // Dash

        tracing::info!(
            "Identity not registered, registering new identity with {} Dash",
            dash
        );

        backend
            .run_task(Task::Identity(IdentityTask::RegisterIdentity(amount)))
            .await;
    }

    let credits_balance = backend
        .state()
        .loaded_identity
        .lock()
        .await
        .as_ref()
        .unwrap()
        .balance();

    tracing::info!("Identity is initialized with {} credits", credits_balance);

    backend.state().save(&backend.config);

    let data_contract = DataContract::fetch(
        &backend.sdk,
        Into::<Identifier>::into(dpns_contract::ID_BYTES),
    )
    .await
    .unwrap()
    .unwrap();

    let document_type = data_contract
        .document_type_cloned_for_name("preorder")
        .unwrap();

    backend
        .state()
        .broadcast_random_documents(
            Arc::clone(&backend.sdk),
            Arc::new(data_contract),
            Arc::new(document_type),
            Duration::from_secs(args.time.into()),
            args.connections,
        )
        .await
        .unwrap();
}
