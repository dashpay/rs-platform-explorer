use std::{
    collections::{BTreeMap, HashSet},
    ops::Deref,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dash_platform_sdk::{
    platform::{transition::put_document::PutDocument, DocumentQuery, FetchMany},
    Error as SdkError, Sdk,
};
use dpp::{
    data_contract::{
        accessors::v0::DataContractV0Getters,
        document_type::{
            accessors::DocumentTypeV0Getters,
            random_document::{CreateRandomDocument, DocumentFieldFillSize, DocumentFieldFillType},
            DocumentType,
        },
    },
    document::{Document, DocumentV0Getters},
    identity::{
        accessors::IdentityGettersV0,
        identity_public_key::accessors::v0::IdentityPublicKeyGettersV0, KeyType, Purpose,
    },
    platform_value::string_encoding::Encoding,
    prelude::DataContract,
};
use futures::future::join_all;
use rand::{prelude::StdRng, Rng, SeedableRng};
use rs_dapi_client::RequestSettings;
use simple_signer::signer::SimpleSigner;
use tokio::{
    sync::{mpsc, Semaphore},
    time,
    time::Instant,
};
use tracing::Level;

use super::{AppStateUpdate, CompletedTaskPayload};
use crate::backend::{error::Error, AppState, BackendEvent, Task};

#[derive(Clone)]
pub(crate) struct BroadcastRandomDocumentsTaskPayload {
    pub data_contract: Arc<DataContract>,
    pub document_type: Arc<DocumentType>,
    pub count: usize,
}

#[derive(Clone)]
pub(crate) enum DocumentTask {
    QueryDocuments(DocumentQuery),
    BroadcastRandomDocuments(BroadcastRandomDocumentsTaskPayload),
}

impl AppState {
    pub(super) async fn run_document_task(
        &self,
        sdk: Arc<Sdk>,
        task: DocumentTask,
    ) -> BackendEvent {
        match &task {
            DocumentTask::QueryDocuments(document_query) => {
                let execution_result = Document::fetch_many(&sdk, document_query.clone())
                    .await
                    .map(CompletedTaskPayload::Documents)
                    .map_err(|e| e.to_string());
                BackendEvent::TaskCompleted {
                    task: Task::Document(task),
                    execution_result,
                }
            }
            DocumentTask::BroadcastRandomDocuments(payload) => {
                let execution_result = self
                    .broadcast_random_documents_and_verify_proofs(
                        Arc::clone(&sdk),
                        Arc::clone(&payload.data_contract),
                        Arc::clone(&payload.document_type),
                        payload.count,
                    )
                    .await
                    .map(CompletedTaskPayload::DocumentBroadcastResults)
                    .map_err(|e| e.to_string());

                if execution_result.is_ok() {
                    match self.refresh_identity(&sdk).await {
                        Ok(updated_identity) => BackendEvent::TaskCompletedStateChange {
                            task: Task::Document(task),
                            execution_result,
                            app_state_update: AppStateUpdate::LoadedIdentity(updated_identity),
                        },
                        Err(_) => BackendEvent::TaskCompletedStateChange {
                            task: Task::Document(task),
                            execution_result,
                            app_state_update: AppStateUpdate::FailedToRefreshIdentity,
                        },
                    }
                } else {
                    BackendEvent::TaskCompleted {
                        task: Task::Document(task),
                        execution_result,
                    }
                }
            }
        }
    }

    pub(crate) async fn broadcast_random_documents_and_then_verify_proofs(
        &self,
        sdk: Arc<Sdk>,
        data_contract: Arc<DataContract>,
        document_type: Arc<DocumentType>,
        count: usize,
    ) -> Result<Vec<Result<Document, SdkError>>, Error> {
        let start_time = Instant::now();

        tracing::info!(
            data_contract_id = data_contract.id().to_string(Encoding::Base58),
            document_type = document_type.name(),
            "broadcasting {count} random documents"
        );

        // Get identity

        let mut loaded_identity = self.loaded_identity.lock().await;
        let Some(identity) = loaded_identity.as_mut() else {
            return Err(Error::IdentityTopUp("No identity loaded".to_string()));
        };

        let identity_id = identity.id();

        // Get identity public key

        let identity_public_key = identity
            .get_first_public_key_matching(
                Purpose::AUTHENTICATION,
                HashSet::from([document_type.security_level_requirement()]),
                HashSet::from([KeyType::ECDSA_SECP256K1, KeyType::BLS12_381]),
            )
            .ok_or(Error::DocumentSigning(
                "No public key matching security level requirements".to_string(),
            ))?;

        // Get the private key to sign state transition

        let loaded_identity_private_keys = self.identity_private_keys.lock().await;

        let Some(private_key) =
            loaded_identity_private_keys.get(&(identity.id(), identity_public_key.id()))
        else {
            return Err(Error::IdentityTopUp(format!(
                "expected private keys, but we only have private keys for {:?}, trying to get \
                 {:?} : {}",
                loaded_identity_private_keys
                    .keys()
                    .map(|(id, key_id)| (id, key_id))
                    .collect::<BTreeMap<_, _>>(),
                identity.id(),
                identity_public_key.id(),
            )));
        };

        let private_key = *private_key;

        // Created time for the documents

        let time_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();

        // Generate and broadcast N documents

        let mut broadcast_transition_futures = Vec::new();

        for _ in 0..count {
            let sdk = Arc::clone(&sdk);
            let data_contract = Arc::clone(&data_contract);
            let document_type = Arc::clone(&document_type);

            let identity_public_key = identity_public_key.clone();

            let document_result = tokio::task::spawn(async move {
                let mut std_rng = StdRng::from_entropy();
                let document_state_transition_entropy: [u8; 32] = std_rng.gen();

                // Generate a random document

                let random_document = document_type
                    .random_document_with_params(
                        identity_id,
                        document_state_transition_entropy.into(),
                        time_ms as u64,
                        DocumentFieldFillType::FillIfNotRequired,
                        DocumentFieldFillSize::AnyDocumentFillSize,
                        &mut std_rng,
                        sdk.version(),
                    )
                    .expect("expected a random document");

                // Create a signer

                let mut signer = SimpleSigner::default();

                signer.add_key(identity_public_key.clone(), private_key.to_bytes());

                // Broadcast the document

                tracing::debug!(
                    data_contract_id = data_contract.id().to_string(Encoding::Base58),
                    document_type = document_type.name(),
                    "broadcasting document {}",
                    random_document.id().to_string(Encoding::Base58),
                );

                let result = random_document
                    .put_to_platform(
                        &sdk,
                        document_type.deref().clone(),
                        document_state_transition_entropy,
                        identity_public_key,
                        &signer,
                    )
                    .await;

                match &result {
                    Ok(_) => tracing::debug!(
                        data_contract_id = data_contract.id().to_string(Encoding::Base58),
                        document_type = document_type.name(),
                        "document {} successfully broadcast",
                        random_document.id().to_string(Encoding::Base58),
                    ),
                    Err(error) => tracing::error!(
                        data_contract_id = data_contract.id().to_string(Encoding::Base58),
                        document_type = document_type.name(),
                        ?error,
                        "failed to broadcast document {}: {}",
                        random_document.id().to_string(Encoding::Base58),
                        error
                    ),
                };

                result.map(|state_transition| (random_document, state_transition))
            });

            broadcast_transition_futures.push(document_result);
        }

        let broadcast_transition_results: Vec<_> = join_all(broadcast_transition_futures)
            .await
            .into_iter()
            // Unwrap panics
            .map(Result::unwrap)
            .collect();

        if tracing::enabled!(Level::INFO) {
            let (oks, errs): (Vec<_>, Vec<_>) =
                broadcast_transition_results.iter().partition(|r| r.is_ok());

            tracing::info!(
                data_contract_id = data_contract.id().to_string(Encoding::Base58),
                document_type = document_type.name(),
                "{count} documents broadcasted in {} secs: {} successfully, {} failed",
                start_time.elapsed().as_secs_f32(),
                oks.len(),
                errs.len(),
            );
        }

        // Wait for the successfully broadcast state transition results

        let mut wait_for_st_result_futures = Vec::new();

        for broadcast_transition_result in broadcast_transition_results {
            let sdk = Arc::clone(&sdk);
            let data_contract = Arc::clone(&data_contract);
            let document_type = Arc::clone(&document_type);

            let wait_for_st_result_future = tokio::task::spawn(async move {
                match broadcast_transition_result {
                    Err(e) => Err(e),
                    Ok((random_document, state_transition)) => {
                        tracing::debug!(
                            data_contract_id = data_contract.id().to_string(Encoding::Base58),
                            document_type = document_type.name(),
                            "waiting for ST results for document {}",
                            random_document.id().to_string(Encoding::Base58),
                        );

                        // TODO: Why do we need full type annotation?
                        let result = <Document as PutDocument<SimpleSigner>>::wait_for_response(
                            &random_document,
                            &sdk,
                            state_transition,
                            Arc::clone(&data_contract),
                        )
                        .await;

                        match &result {
                            Ok(_) => tracing::debug!(
                                data_contract_id = data_contract.id().to_string(Encoding::Base58),
                                document_type = document_type.name(),
                                "document {} successfully created",
                                random_document.id().to_string(Encoding::Base58),
                            ),
                            Err(error) => tracing::error!(
                                data_contract_id = data_contract.id().to_string(Encoding::Base58),
                                document_type = document_type.name(),
                                ?error,
                                "failed verify document creation {}: {}",
                                random_document.id().to_string(Encoding::Base58),
                                error,
                            ),
                        };

                        result
                    }
                }
            });

            wait_for_st_result_futures.push(wait_for_st_result_future);
        }

        let wait_for_st_results: Vec<_> = join_all(wait_for_st_result_futures)
            .await
            .into_iter()
            // Unwrap panics
            .map(Result::unwrap)
            .collect();

        if tracing::enabled!(Level::INFO) {
            let (oks, errs): (Vec<_>, Vec<_>) = wait_for_st_results.iter().partition(|r| r.is_ok());

            tracing::info!(
                data_contract_id = data_contract.id().to_string(Encoding::Base58),
                document_type = document_type.name(),
                "received {} broadcast document results in {}: {} successfully, {} failed",
                count,
                start_time.elapsed().as_secs_f32(),
                oks.len(),
                errs.len(),
            );
        }

        Ok(wait_for_st_results)
    }

    pub(crate) async fn broadcast_random_documents_and_verify_proofs(
        &self,
        sdk: Arc<Sdk>,
        data_contract: Arc<DataContract>,
        document_type: Arc<DocumentType>,
        count: usize,
    ) -> Result<Vec<Result<Document, SdkError>>, Error> {
        let start_time = Instant::now();

        tracing::info!(
            data_contract_id = data_contract.id().to_string(Encoding::Base58),
            document_type = document_type.name(),
            "broadcasting {count} random documents"
        );

        // Get identity

        let mut loaded_identity = self.loaded_identity.lock().await;
        let Some(identity) = loaded_identity.as_mut() else {
            return Err(Error::IdentityTopUp("No identity loaded".to_string()));
        };

        let identity_id = identity.id();

        // Get identity public key

        let identity_public_key = identity
            .get_first_public_key_matching(
                Purpose::AUTHENTICATION,
                HashSet::from([document_type.security_level_requirement()]),
                HashSet::from([KeyType::ECDSA_SECP256K1, KeyType::BLS12_381]),
            )
            .ok_or(Error::DocumentSigning(
                "No public key matching security level requirements".to_string(),
            ))?;

        // Get the private key to sign state transition

        let loaded_identity_private_keys = self.identity_private_keys.lock().await;

        let Some(private_key) =
            loaded_identity_private_keys.get(&(identity.id(), identity_public_key.id()))
        else {
            return Err(Error::IdentityTopUp(format!(
                "expected private keys, but we only have private keys for {:?}, trying to get \
                 {:?} : {}",
                loaded_identity_private_keys
                    .keys()
                    .map(|(id, key_id)| (id, key_id))
                    .collect::<BTreeMap<_, _>>(),
                identity.id(),
                identity_public_key.id(),
            )));
        };

        let private_key = *private_key;

        // Created time for the documents

        let time_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();

        // Generate and broadcast N documents

        let mut broadcast_transition_futures = Vec::new();

        let permits = Arc::new(Semaphore::new(500));

        for _ in 0..count {
            let permits = Arc::clone(&permits);
            let sdk = Arc::clone(&sdk);
            let data_contract = Arc::clone(&data_contract);
            let document_type = Arc::clone(&document_type);

            let identity_public_key = identity_public_key.clone();

            let document_result = tokio::task::spawn(async move {
                let mut std_rng = StdRng::from_entropy();
                let document_state_transition_entropy: [u8; 32] = std_rng.gen();

                // Generate a random document

                let random_document = document_type
                    .random_document_with_params(
                        identity_id,
                        document_state_transition_entropy.into(),
                        time_ms as u64,
                        DocumentFieldFillType::FillIfNotRequired,
                        DocumentFieldFillSize::AnyDocumentFillSize,
                        &mut std_rng,
                        sdk.version(),
                    )
                    .expect("expected a random document");

                // Create a signer

                let mut signer = SimpleSigner::default();

                signer.add_key(identity_public_key.clone(), private_key.to_bytes());

                // Broadcast the document

                tracing::debug!(
                    data_contract_id = data_contract.id().to_string(Encoding::Base58),
                    document_type = document_type.name(),
                    "broadcasting document {}",
                    random_document.id().to_string(Encoding::Base58),
                );

                let permit = permits.acquire_owned().await.expect("should acquire");

                let result = random_document
                    .put_to_platform_and_wait_for_response(
                        &sdk,
                        document_type.deref().clone(),
                        document_state_transition_entropy,
                        identity_public_key,
                        Arc::clone(&data_contract),
                        &signer,
                    )
                    .await;

                drop(permit);

                match &result {
                    Ok(_) => tracing::debug!(
                        data_contract_id = data_contract.id().to_string(Encoding::Base58),
                        document_type = document_type.name(),
                        "document {} successfully created",
                        random_document.id().to_string(Encoding::Base58),
                    ),
                    Err(error) => tracing::error!(
                        data_contract_id = data_contract.id().to_string(Encoding::Base58),
                        document_type = document_type.name(),
                        ?error,
                        "failed to verify document creation {}: {}",
                        random_document.id().to_string(Encoding::Base58),
                        error
                    ),
                };

                result
            });

            broadcast_transition_futures.push(document_result);
        }

        let broadcast_transition_results: Vec<_> = join_all(broadcast_transition_futures)
            .await
            .into_iter()
            // Unwrap panics
            .map(Result::unwrap)
            .collect();

        if tracing::enabled!(Level::INFO) {
            let (oks, errs): (Vec<_>, Vec<_>) =
                broadcast_transition_results.iter().partition(|r| r.is_ok());

            tracing::info!(
                data_contract_id = data_contract.id().to_string(Encoding::Base58),
                document_type = document_type.name(),
                "received {} broadcast document results in {}: {} successfully, {} failed",
                count,
                start_time.elapsed().as_secs_f32(),
                oks.len(),
                errs.len(),
            );
        }

        Ok(broadcast_transition_results)
    }

    pub async fn broadcast_random_documents(
        &self,
        sdk: Arc<Sdk>,
        data_contract: Arc<DataContract>,
        document_type: Arc<DocumentType>,
        duration: Duration,
        concurrent_requests: u16,
    ) -> Result<(), Error> {
        tracing::info!(
            data_contract_id = data_contract.id().to_string(Encoding::Base58),
            document_type = document_type.name(),
            "broadcasting simultaneously {} random documents for {} secs",
            concurrent_requests,
            duration.as_secs_f32()
        );

        // Get identity

        let mut loaded_identity = self.loaded_identity.lock().await;
        let Some(identity) = loaded_identity.as_mut() else {
            return Err(Error::IdentityTopUp("No identity loaded".to_string()));
        };

        let identity_id = identity.id();

        // Get identity public key

        let identity_public_key = identity
            .get_first_public_key_matching(
                Purpose::AUTHENTICATION,
                HashSet::from([document_type.security_level_requirement()]),
                HashSet::from([KeyType::ECDSA_SECP256K1, KeyType::BLS12_381]),
            )
            .ok_or(Error::DocumentSigning(
                "No public key matching security level requirements".to_string(),
            ))?;

        // Get the private key to sign state transition

        let loaded_identity_private_keys = self.identity_private_keys.lock().await;

        let Some(private_key) =
            loaded_identity_private_keys.get(&(identity.id(), identity_public_key.id()))
        else {
            return Err(Error::IdentityTopUp(format!(
                "expected private keys, but we only have private keys for {:?}, trying to get \
                 {:?} : {}",
                loaded_identity_private_keys
                    .keys()
                    .map(|(id, key_id)| (id, key_id))
                    .collect::<BTreeMap<_, _>>(),
                identity.id(),
                identity_public_key.id(),
            )));
        };

        let private_key = *private_key;

        // Created time for the documents

        let created_at_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();

        // Generate and broadcast N documents

        let permits = Arc::new(Semaphore::new(concurrent_requests as usize));

        let oks = Arc::new(AtomicUsize::new(0)); // Atomic counter for tasks
        let errs = Arc::new(AtomicUsize::new(0)); // Atomic counter for tasks
        let pending = Arc::new(AtomicUsize::new(0));
        let last_report = Arc::new(AtomicU64::new(0));

        let start_time = Instant::now();

        let mut tasks = Vec::new();

        let settings = RequestSettings {
            connect_timeout: None,
            timeout: Some(Duration::from_secs(30)),
            retries: Some(0),
        };

        while start_time.elapsed() < duration {
            // Acquire a permit
            let permits = Arc::clone(&permits);
            let permit = permits.acquire_owned().await.unwrap();

            let oks = Arc::clone(&oks);
            let errs = Arc::clone(&errs);
            let pending = Arc::clone(&pending);
            let last_report = Arc::clone(&last_report);

            let sdk = Arc::clone(&sdk);
            let document_type = Arc::clone(&document_type);

            let identity_public_key = identity_public_key.clone();

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
                        sdk.version(),
                    )
                    .expect("expected a random document");

                // Create a signer

                let mut signer = SimpleSigner::default();

                signer.add_key(identity_public_key.clone(), private_key.to_bytes());

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
                    .put_to_platform_with_settings(
                        &sdk,
                        document_type.deref().clone(),
                        document_state_transition_entropy,
                        identity_public_key,
                        &signer,
                        settings.clone(),
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
            "broadcasting {} random documents during {} secs. successfully: {}, failed: {}, rate: \
             {} docs/sec",
            oks + errs,
            duration.as_secs_f32(),
            oks,
            errs,
            (oks + errs) as f32 / duration.as_secs_f32()
        );

        Ok(())
    }
}
