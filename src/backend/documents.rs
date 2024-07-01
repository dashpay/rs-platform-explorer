use dash_sdk::platform::transition::vote::PutVote;
use dash_sdk::{
    platform::{transition::put_document::PutDocument, DocumentQuery, FetchMany},
    Sdk,
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
    document::Document,
    identifier::Identifier,
    identity::{
        accessors::IdentityGettersV0,
        identity_public_key::accessors::v0::IdentityPublicKeyGettersV0, KeyType, Purpose,
        SecurityLevel,
    },
    platform_value::Value,
    prelude::{DataContract, Identity, IdentityPublicKey},
    voting::{
        contender_structs::ContenderWithSerializedDocument,
        vote_choices::resource_vote_choice::ResourceVoteChoice,
        vote_polls::{
            contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll, VotePoll,
        },
        votes::{resource_vote::ResourceVote, Vote},
    },
};
use drive::query::{
    vote_poll_vote_state_query::{
        ContestedDocumentVotePollDriveQuery, ContestedDocumentVotePollDriveQueryResultType,
    },
    vote_polls_by_document_type_query::VotePollsByDocumentTypeQuery,
};
use drive_proof_verifier::types::ContestedResource;
use futures::{stream::FuturesUnordered, Future, StreamExt};
use rand::{prelude::StdRng, Rng, SeedableRng};
use simple_signer::signer::SimpleSigner;
use std::{
    collections::{BTreeMap, HashSet},
    iter,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{state::IdentityPrivateKeysMap, AppStateUpdate, CompletedTaskPayload};
use crate::backend::{error::Error, AppState, BackendEvent, Task};

#[derive(Debug, Clone)]
pub(crate) enum DocumentTask {
    QueryDocuments(DocumentQuery),
    BroadcastRandomDocuments {
        data_contract_name: String,
        document_type_name: String,
        count: u16,
    },
    QueryContestedResources(DataContract, DocumentType),
    QueryVoteContenders(String, Vec<Value>, String, Identifier),
    VoteOnContestedResource(VotePoll, ResourceVoteChoice),
}

impl AppState {
    pub(super) async fn run_document_task<'s>(
        &'s self,
        sdk: &Sdk,
        task: DocumentTask,
    ) -> BackendEvent<'s> {
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
            DocumentTask::BroadcastRandomDocuments {
                data_contract_name,
                document_type_name,
                count,
            } => {
                let broadcast_stats = {
                    let data_contracts_lock = self.known_contracts.lock().await;
                    let Some(data_contract) = data_contracts_lock.get(data_contract_name) else {
                        return BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Err("Data contract not found".to_owned()),
                        };
                    };
                    let loaded_identity_lock = self.loaded_identity.lock().await;
                    let Some(identity) = loaded_identity_lock.as_ref() else {
                        return BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Err("No loaded identity".to_owned()),
                        };
                    };
                    let identity_private_keys_lock = self.identity_private_keys.lock().await;
                    let Ok(document_type) =
                        data_contract.document_type_cloned_for_name(&document_type_name)
                    else {
                        return BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Err("Document type not found".to_owned()),
                        };
                    };

                    broadcast_random_documents(
                        sdk,
                        identity,
                        &identity_private_keys_lock,
                        data_contract,
                        &document_type,
                        *count,
                    )
                    .await
                };

                match broadcast_stats {
                    Ok(stats) => match self.refresh_identity(sdk).await {
                        Ok(updated_identity) => BackendEvent::TaskCompletedStateChange {
                            task: Task::Document(task),
                            execution_result: Ok(CompletedTaskPayload::String(
                                stats.info_display(),
                            )),
                            app_state_update: AppStateUpdate::LoadedIdentity(updated_identity),
                        },
                        Err(_) => BackendEvent::TaskCompletedStateChange {
                            task: Task::Document(task),
                            execution_result: Ok(CompletedTaskPayload::String(
                                stats.info_display(),
                            )),
                            app_state_update: AppStateUpdate::FailedToRefreshIdentity,
                        },
                    },
                    Err(e) => {
                        return BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Err(format!(
                                "Unable to start broadcasting: {}",
                                e.to_string()
                            )),
                        }
                    }
                }
            }
            DocumentTask::QueryVoteContenders(
                index_name,
                index_values,
                document_type_name,
                contract_id,
            ) => {
                let query = ContestedDocumentVotePollDriveQuery {
                    limit: None,
                    offset: None,
                    start_at: None,
                    vote_poll: ContestedDocumentResourceVotePoll {
                        index_name: index_name.to_string(),
                        index_values: index_values.to_vec(),
                        document_type_name: document_type_name.to_string(),
                        contract_id: *contract_id,
                    },
                    allow_include_locked_and_abstaining_vote_tally: true,
                    result_type:
                        ContestedDocumentVotePollDriveQueryResultType::DocumentsAndVoteTally,
                };

                let contenders =
                    match ContenderWithSerializedDocument::fetch_many(sdk, query.clone()).await {
                        Ok(contenders) => contenders,
                        Err(e) => {
                            return BackendEvent::TaskCompleted {
                                task: Task::Document(task),
                                execution_result: Err(format!("{e}")),
                            }
                        }
                    };

                BackendEvent::TaskCompleted {
                    task: Task::Document(task),
                    execution_result: Ok(CompletedTaskPayload::ContestedResourceContenders(
                        query.vote_poll,
                        contenders,
                    )),
                }
            }
            DocumentTask::QueryContestedResources(data_contract, document_type) => {
                if let Some(contested_index) = document_type.find_contested_index() {
                    let query = VotePollsByDocumentTypeQuery {
                        contract_id: data_contract.id(),
                        document_type_name: document_type.name().to_string(),
                        index_name: contested_index.name.clone(),
                        start_at_value: None,
                        start_index_values: vec!["dash".into()], // hardcoded for dpns
                        end_index_values: vec![],
                        limit: None,
                        order_ascending: true,
                    };

                    let contested_resources = ContestedResource::fetch_many(sdk, query).await;

                    match contested_resources {
                        Ok(resources) => BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Ok(CompletedTaskPayload::ContestedResources(
                                resources,
                            )),
                        },
                        Err(e) => BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Err(format!("{e}")),
                        },
                    }
                } else {
                    BackendEvent::TaskCompleted {
                        task: Task::Document(task),
                        execution_result: Err(
                            "No contested index for this document type".to_owned()
                        ),
                    }
                }
            }
            DocumentTask::VoteOnContestedResource(vote_poll, vote_choice) => {
                let mut vote = Vote::default();

                // Get signer from loaded_identity
                // Convert loaded_identity to SimpleSigner
                let identity_private_keys_lock = self.identity_private_keys.lock().await;
                let loaded_identity_lock = self
                    .loaded_identity
                    .lock()
                    .await
                    .clone()
                    .expect("Expected to get a loaded identity");

                let mut signer = SimpleSigner::default();
                let Identity::V0(identity_v0) = &loaded_identity_lock;
                for (key_id, public_key) in &identity_v0.public_keys {
                    let identity_key_tuple = (identity_v0.id, *key_id);
                    if let Some(private_key_bytes) =
                        identity_private_keys_lock.get(&identity_key_tuple)
                    {
                        signer
                            .private_keys
                            .insert(public_key.clone(), private_key_bytes.clone());
                    }
                }

                let voting_public_key = match loaded_identity_lock.get_first_public_key_matching(
                    Purpose::VOTING,
                    HashSet::from(SecurityLevel::full_range()),
                    HashSet::from(KeyType::all_key_types()),
                ) {
                    Some(voting_key) => voting_key,
                    None => {
                        return BackendEvent::TaskCompleted {
                            task: Task::Document(task),
                            execution_result: Err(
                                "No voting key in the loaded identity. Are you sure it's a masternode identity?".to_string()
                            ),
                        };
                    }
                };

                match vote {
                    Vote::ResourceVote(ref mut resource_vote) => match resource_vote {
                        ResourceVote::V0(ref mut resource_vote_v0) => {
                            resource_vote_v0.vote_poll = vote_poll.clone();
                            resource_vote_v0.resource_vote_choice = *vote_choice;
                            let pro_tx_hash = self
                                .loaded_identity_pro_tx_hash
                                .lock()
                                .await
                                .expect("Expected a proTxHash in AppState");
                            match vote
                                .put_to_platform_and_wait_for_response(
                                    pro_tx_hash,
                                    voting_public_key,
                                    sdk,
                                    &signer,
                                    None,
                                )
                                .await
                            {
                                Ok(_) => BackendEvent::TaskCompleted {
                                    task: Task::Document(task),
                                    execution_result: Ok(CompletedTaskPayload::String(
                                        "Vote cast successfully".to_string(),
                                    )),
                                },
                                Err(e) => BackendEvent::TaskCompleted {
                                    task: Task::Document(task),
                                    execution_result: Err(e.to_string()),
                                },
                            }
                        }
                    },
                }
            }
        }
    }
}

struct BroadcastRandomDocumentsStats {
    total: u16,
    completed: u16,
    last_error: Option<String>,
}

impl BroadcastRandomDocumentsStats {
    fn info_display(&self) -> String {
        format!(
            "Broadcast random documents results:
Completed {} of {}
Last error: {}",
            self.completed,
            self.total,
            self.last_error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or("".to_owned())
        )
    }
}

async fn broadcast_random_documents<'s>(
    sdk: &Sdk,
    identity: &Identity,
    identity_private_keys: &IdentityPrivateKeysMap,
    data_contract: &DataContract,
    document_type: &DocumentType,
    count: u16,
) -> Result<BroadcastRandomDocumentsStats, Error> {
    let mut std_rng = StdRng::from_entropy();

    let identity_public_key = identity
        .get_first_public_key_matching(
            Purpose::AUTHENTICATION,
            HashSet::from([document_type.security_level_requirement()]),
            HashSet::from([KeyType::ECDSA_SECP256K1, KeyType::BLS12_381]),
        )
        .ok_or(Error::DocumentSigningError(
            "No public key matching security level requirements".to_string(),
        ))?;

    let Some(private_key) = identity_private_keys.get(&(identity.id(), identity_public_key.id()))
    else {
        // TODO inappropriate error type
        return Err(Error::IdentityTopUpError(format!(
            "expected private keys, but we only have private keys for {:?}, trying to get {:?} : \
             {}",
            identity_private_keys
                .keys()
                .map(|(id, key_id)| (id, key_id))
                .collect::<BTreeMap<_, _>>(),
            identity.id(),
            identity_public_key.id(),
        )));
    };

    let data_contract = Arc::new(data_contract.clone());
    let mut signer = SimpleSigner::default();
    signer.add_key(identity_public_key.clone(), private_key.to_vec());

    fn put_random_document<'a, 'r>(
        sdk: &'a Sdk,
        document_type: &'a DocumentType,
        identity: &'a Identity,
        rng: &'r mut StdRng,
        signer: &'a SimpleSigner,
        identity_public_key: &'a IdentityPublicKey,
        data_contract: Arc<DataContract>,
    ) -> impl Future<Output = Result<(), String>> + 'a {
        let document_state_transition_entropy: [u8; 32] = rng.gen();
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock may have gone backwards")
            .as_millis();

        let random_document = document_type
            .random_document_with_params(
                identity.id(),
                document_state_transition_entropy.into(),
                Some(time_ms as u64),
                None,
                None,
                DocumentFieldFillType::FillIfNotRequired,
                DocumentFieldFillSize::AnyDocumentFillSize,
                rng,
                sdk.version(),
            )
            .expect("expected a random document");

        async move {
            random_document
                .put_to_platform_and_wait_for_response(
                    sdk,
                    document_type.clone(),
                    document_state_transition_entropy,
                    identity_public_key.clone(),
                    data_contract,
                    signer,
                )
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }

    let mut futures: FuturesUnordered<_> = iter::repeat_with(|| {
        put_random_document(
            sdk,
            document_type,
            identity,
            &mut std_rng,
            &signer,
            identity_public_key,
            Arc::clone(&data_contract),
        )
    })
    .take(count as usize)
    .collect();

    let mut completed = 0;
    let mut last_error = None;

    while let Some(result) = futures.next().await {
        match result {
            Ok(_) => completed += 1,
            Err(e) => last_error = Some(e),
        }
    }

    Ok(BroadcastRandomDocumentsStats {
        total: count,
        completed,
        last_error,
    })
}
