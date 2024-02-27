use std::{
    collections::HashSet,
    num::NonZeroU32,
    panic,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use dpp::{
    data_contract::{
        accessors::v0::DataContractV0Getters,
        document_type::{
            accessors::DocumentTypeV0Getters,
            random_document::{CreateRandomDocument, DocumentFieldFillSize, DocumentFieldFillType},
            DocumentType,
        },
        DataContract,
    },
    data_contracts::dpns_contract,
    document::DocumentV0Getters,
    identity::{
        accessors::IdentityGettersV0,
        identity_public_key::accessors::v0::IdentityPublicKeyGettersV0, Identity, KeyType, Purpose,
    },
    platform_value::string_encoding::Encoding,
    version::PlatformVersion,
};
use futures::future::join_all;
use governor::{Quota, RateLimiter};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rs_dapi_client::RequestSettings;
use rs_platform_explorer::{
    backend::{
        identities::IdentityTask, insight::InsightAPIClient, state::IdentityPrivateKeysMap,
        wallet::WalletTask, Backend, Task,
    },
    config::Config,
};
use rs_sdk::{
    platform::{
        transition::put_document::{PutDocument, PutSettings},
        Fetch, Identifier,
    },
    Sdk, SdkBuilder,
};
use simple_signer::signer::SimpleSigner;
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;
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
    #[arg(
        short,
        long,
        help = "Number of transactions to send per second",
        default_value = "0"
    )]
    rate: u32,
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

    let sdk = SdkBuilder::new(address_list)
        .with_version(PlatformVersion::get(1).unwrap())
        .with_core(
            &config.core_host,
            config.core_rpc_port,
            &config.core_rpc_user,
            &config.core_rpc_password,
        )
        .build()
        .expect("expected to build sdk");

    let insight = InsightAPIClient::new(config.insight_api_uri());

    let backend = Backend::new(sdk.as_ref(), insight, config.clone()).await;

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

    let identity_lock = backend.state().loaded_identity.lock().await;
    let identity = identity_lock.as_ref().expect("no loaded identity");

    let identity_private_keys_lock = backend.state().identity_private_keys.lock().await;

    broadcast_random_documents_load_test(
        Arc::clone(&sdk),
        &identity,
        &identity_private_keys_lock,
        Arc::new(data_contract),
        Arc::new(document_type),
        Duration::from_secs(args.time.into()),
        args.connections,
        args.rate,
    )
    .await;
}

async fn broadcast_random_documents_load_test(
    sdk: Arc<Sdk>,
    identity: &Identity,
    identity_private_keys: &IdentityPrivateKeysMap,
    data_contract: Arc<DataContract>,
    document_type: Arc<DocumentType>,
    duration: Duration,
    concurrent_requests: u16,
    rate_limit_per_sec: u32,
) {
    let rate_limit_per_sec = NonZeroU32::new(rate_limit_per_sec).unwrap_or(NonZeroU32::MAX);
    tracing::info!(
        data_contract_id = data_contract.id().to_string(Encoding::Base58),
        document_type = document_type.name(),
        "broadcasting up to {} random documents per second in {} parallel threads for {} secs",
        rate_limit_per_sec,
        concurrent_requests,
        duration.as_secs_f32()
    );

    let identity_id = identity.id();

    // Get identity public key

    let identity_public_key = identity
        .get_first_public_key_matching(
            Purpose::AUTHENTICATION,
            HashSet::from([document_type.security_level_requirement()]),
            HashSet::from([KeyType::ECDSA_SECP256K1, KeyType::BLS12_381]),
        )
        .expect("No public key matching security level requirements");

    // Get the private key to sign state transition

    let private_key: Vec<u8> = identity_private_keys
        .get(&(identity.id(), identity_public_key.id()))
        .expect("expected private keys")
        .clone();

    // Created time for the documents

    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis();

    // Generate and broadcast N documents

    let permits = Arc::new(Semaphore::new(concurrent_requests as usize));

    let rate_limit = Arc::new(RateLimiter::direct(Quota::per_second(rate_limit_per_sec)));

    let cancel = CancellationToken::new();

    // what the hell
    let oks = Arc::new(AtomicUsize::new(0)); // Atomic counter for tasks
    let errs = Arc::new(AtomicUsize::new(0)); // Atomic counter for tasks
    let pending = Arc::new(AtomicUsize::new(0));
    let last_report = Arc::new(AtomicU64::new(0));

    let start_time = Instant::now();

    let mut tasks = Vec::new();

    let settings = RequestSettings {
        connect_timeout: Some(Duration::from_secs(60)),
        timeout: Some(Duration::from_secs(30)),
        retries: Some(0),
        ban_failed_address: Some(false),
    };

    // start a timer to cancel the broadcast after the duration
    let timeout_cancel = cancel.clone();
    tokio::task::spawn(async move {
        tokio::select! {
            _ = timeout_cancel.cancelled() => {},
            _ = tokio::time::sleep_until(start_time + duration) => {},
        }

        tracing::info!("cancelling the broadcast of random documents");
        timeout_cancel.cancel()
    });

    let version = sdk.version();

    while !cancel.is_cancelled() {
        // Acquire a permit
        let permits = Arc::clone(&permits);
        let permit = permits.acquire_owned().await.unwrap();

        let oks = Arc::clone(&oks);
        let errs = Arc::clone(&errs);
        let pending = Arc::clone(&pending);
        let last_report = Arc::clone(&last_report);

        let document_type = Arc::clone(&document_type);

        let identity_public_key = identity_public_key.clone();

        let rate_limiter = rate_limit.clone();
        let cancel_task = cancel.clone();
        let sdk = Arc::clone(&sdk);
        let private_key = private_key.clone();

        let task = tokio::task::spawn(async move {
            let mut std_rng = StdRng::from_entropy();
            let document_state_transition_entropy: [u8; 32] = std_rng.gen();

            // Generate a random document

            let random_document = document_type
                .random_document_with_params(
                    identity_id,
                    document_state_transition_entropy.into(),
                    created_at_ms as u64,
                    DocumentFieldFillType::FillIfNotRequired,
                    DocumentFieldFillSize::AnyDocumentFillSize,
                    &mut std_rng,
                    version,
                )
                .expect("expected a random document");

            // Create a signer

            let mut signer = SimpleSigner::default();

            signer.add_key(identity_public_key.clone(), private_key.clone());

            // Wait for the rate limiter to allow further processing
            tokio::select! {
               _ = rate_limiter.until_ready() => {},
               _ = cancel_task.cancelled() => return,
            };

            // Broadcast the document
            tracing::trace!(
                "broadcasting document {}",
                random_document.id().to_string(Encoding::Base58),
            );

            pending.fetch_add(1, Ordering::SeqCst);

            let elapsed_secs = start_time.elapsed().as_secs();

            if start_time.elapsed().as_secs() % 10 == 0
                && elapsed_secs != last_report.load(Ordering::SeqCst)
            {
                tracing::info!(
                    "{} secs passed: {} pending, {} successful, {} failed",
                    elapsed_secs,
                    pending.load(Ordering::SeqCst),
                    oks.load(Ordering::SeqCst),
                    errs.load(Ordering::SeqCst),
                );
                last_report.swap(elapsed_secs, Ordering::SeqCst);
            }

            let result = random_document
                .put_to_platform(
                    &sdk,
                    document_type.as_ref().clone(),
                    document_state_transition_entropy,
                    identity_public_key,
                    &signer,
                    Some(PutSettings {
                        request_settings: settings,
                        identity_nonce_stale_time_s: None,
                    }),
                )
                .await;

            pending.fetch_sub(1, Ordering::SeqCst);

            match result {
                Ok(_) => {
                    oks.fetch_add(1, Ordering::SeqCst);

                    tracing::trace!(
                        "document {} successfully broadcast",
                        random_document.id().to_string(Encoding::Base58),
                    );
                }
                Err(error) => {
                    tracing::error!(
                        ?error,
                        "failed to broadcast document {}: {}",
                        random_document.id().to_string(Encoding::Base58),
                        error
                    );

                    errs.fetch_add(1, Ordering::SeqCst);
                }
            };

            drop(permit);
        });

        tasks.push(task)
    }

    join_all(tasks).await;

    let oks = oks.load(Ordering::SeqCst);
    let errs = errs.load(Ordering::SeqCst);

    tracing::info!(
        data_contract_id = data_contract.id().to_string(Encoding::Base58),
        document_type = document_type.name(),
        "broadcasting {} random documents during {} secs. successfully: {}, failed: {}, rate: {} \
         docs/sec",
        oks + errs,
        duration.as_secs_f32(),
        oks,
        errs,
        (oks + errs) as f32 / duration.as_secs_f32()
    );
}