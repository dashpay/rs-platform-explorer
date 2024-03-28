//! Strategies management backend module.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque}, fs::File, io::Write, time::Instant
};

use dapi_grpc::platform::v0::{
    get_epochs_info_request, get_epochs_info_response,
    wait_for_state_transition_result_response::{
        self, wait_for_state_transition_result_response_v0,
    },
    GetEpochsInfoRequest,
};
use dpp::{
    block::{block_info::BlockInfo, epoch::Epoch}, dashcore::PrivateKey, data_contract::{
        accessors::v0::DataContractV0Getters, created_data_contract::CreatedDataContract, DataContract
    }, identity::{
        accessors::IdentityGettersV0, state_transition::asset_lock_proof::AssetLockProof, Identity, KeyType, PartialIdentity, Purpose, SecurityLevel
    }, platform_value::{string_encoding::Encoding, Identifier}, serialization::{PlatformDeserializableWithPotentialValidationFromVersionedStructure, PlatformSerializableWithPlatformVersion}, state_transition::{data_contract_create_transition::DataContractCreateTransition, documents_batch_transition::{document_base_transition::v0::v0_methods::DocumentBaseTransitionV0Methods, document_transition::DocumentTransition, DocumentCreateTransition, DocumentsBatchTransition}, StateTransition, StateTransitionLike}, version::PlatformVersion
};
use drive::{
    drive::{
        document::query::{QueryDocumentsOutcome, QueryDocumentsOutcomeV0Methods},
        identity::key::fetch::IdentityKeysRequest, Drive,
    }, error::proof::ProofError, query::DriveQuery
};
use futures::future::join_all;
use rand::{rngs::StdRng, SeedableRng};
use rs_dapi_client::{DapiRequest, DapiRequestExecutor, RequestSettings};
use rs_sdk::{
    platform::{transition::broadcast_request::BroadcastRequestForStateTransition, Fetch},
    Sdk,
};
use simple_signer::signer::SimpleSigner;
use strategy_tests::{
    frequency::Frequency, operations::{FinalizeBlockOperation, Operation}, IdentityInsertInfo, KeyMaps, LocalDocumentQuery, StartIdentities, Strategy, StrategyConfig
};
use tokio::sync::{Mutex, MutexGuard};
use tracing::{error, info};

use super::{
    insight::InsightAPIClient, state::{ContractFileName, KnownContractsMap}, AppState, AppStateUpdate,
    BackendEvent, StrategyCompletionResult, StrategyContractNames,
};
use crate::backend::Wallet;

#[derive(Debug, PartialEq, Clone)]
pub enum StrategyTask {
    CreateStrategy(String),
    ImportStrategy(String),
    ExportStrategy(String),
    SelectStrategy(String),
    DeleteStrategy(String),
    CloneStrategy(String),
    SetContractsWithUpdates(String, Vec<String>),
    SetIdentityInserts {
        strategy_name: String,
        identity_inserts_frequency: Frequency,
    },
    SetStartIdentities {
        strategy_name: String,
        count: u8,
        keys_count: u8,
        balance: u64,
        add_transfer_key: bool,
    },
    SetStartIdentitiesBalance(String, u64),
    AddOperation {
        strategy_name: String,
        operation: Operation,
    },
    RunStrategy(String, u64, bool),
    RemoveLastContract(String),
    RemoveIdentityInserts(String),
    RemoveStartIdentities(String),
    RemoveLastOperation(String),
}

pub async fn run_strategy_task<'s>(
    sdk: &Sdk,
    app_state: &'s AppState,
    task: StrategyTask,
    insight: &'s InsightAPIClient,
) -> BackendEvent<'s> {
    match task {
        StrategyTask::CreateStrategy(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;
            let mut selected_strategy_lock = app_state.selected_strategy.lock().await;

            strategies_lock.insert(strategy_name.clone(), Strategy::default());
            *selected_strategy_lock = Some(strategy_name.clone());
            contract_names_lock.insert(strategy_name.clone(), Default::default());

            BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                strategy_name.clone(),
                MutexGuard::map(strategies_lock, |strategies| {
                    strategies.get_mut(&strategy_name).expect("strategy exists")
                }),
                MutexGuard::map(contract_names_lock, |names| {
                    names.get_mut(&strategy_name).expect("inconsistent data")
                }),
            ))
        }
        StrategyTask::ImportStrategy(url) => {
            let platform_version = PlatformVersion::latest();
        
            match reqwest::get(&url).await {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.bytes().await {
                            Ok(bytes) => {
                                match Strategy::versioned_deserialize(&bytes, true, &platform_version) {
                                    Ok(strategy) => {
                                        let strategy_name = url.split('/').last()
                                            .map(|s| s.rsplit_once('.').map_or(s, |(name, _)| name))
                                            .map(|s| s.to_string())
                                            .expect("Expected to extract the filename from the imported Strategy file");
                                                                        
                                        let mut strategies_lock = app_state.available_strategies.lock().await;
                                        strategies_lock.insert(strategy_name.clone(), strategy.clone());

                                        // We need to add the contracts to available_strategies_contract_names so they can be displayed.
                                        // In order to do so, we need to convert contracts_with_updates into Base58-encoded IDs
                                        let mut strategy_contracts_with_updates_in_format: StrategyContractNames = Vec::new();
                                        for (contract, maybe_updates) in strategy.contracts_with_updates {
                                            let contract_name = contract.data_contract().id().to_string(Encoding::Base58);
                                            if let Some(update_map) = maybe_updates {
                                                let formatted_update_map = update_map.into_iter().map(|(block_number, created_contract)| {
                                                    let contract_name = created_contract.data_contract().id().to_string(Encoding::Base58);
                                                    (block_number, contract_name)
                                                }).collect::<BTreeMap<u64, ContractFileName>>();

                                                strategy_contracts_with_updates_in_format.push((contract_name, Some(formatted_update_map)));
                                            } else {
                                                strategy_contracts_with_updates_in_format.push((contract_name, None));
                                            }
                                        }

                                        let mut contract_names_lock = app_state.available_strategies_contract_names.lock().await;
                                        contract_names_lock.insert(strategy_name.clone(), strategy_contracts_with_updates_in_format);

                                        BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                                            strategy_name.clone(),
                                            MutexGuard::map(strategies_lock, |strategies| {
                                                strategies.get_mut(&strategy_name).expect("Expected to find the strategy in available_strategies")
                                            }),
                                            MutexGuard::map(contract_names_lock, |names| {
                                                names.get_mut(&strategy_name).expect("Expected to find the strategy in available_strategies_contract_names")
                                            }),
                                        ))
                                    },
                                    Err(e) => {
                                        error!("Failed to deserialize strategy: {}", e);
                                        BackendEvent::None
                                    }
                                }
                            },
                            Err(e) => {
                                error!("Failed to fetch strategy data: {}", e);
                                BackendEvent::None
                            }
                        }
                    } else {
                        error!("Failed to fetch strategy: HTTP {}", response.status());
                        BackendEvent::None
                    }
                },
                Err(e) => {
                    error!("Failed to fetch strategy: {}", e);
                    BackendEvent::None
                }
            }
        }
        StrategyTask::ExportStrategy(ref strategy_name) => {
            let strategies_lock = app_state.available_strategies.lock().await;
            let strategy = strategies_lock.get(strategy_name)
                .expect("Strategy name doesn't exist in app_state.available_strategies");
            let platform_version = PlatformVersion::latest();
        
            match strategy.serialize_to_bytes_with_platform_version(&platform_version) {
                Ok(binary_data) => {
                    let file_name = format!("supporting_files/strategy_exports/{}", strategy_name);
                    let path = std::path::Path::new(&file_name);
        
                    match File::create(&path) {
                        Ok(mut file) => {
                            if let Err(e) = file.write_all(&binary_data) {
                                error!("Failed to write strategy to file: {}", e);
                            }
                            BackendEvent::None
                        },
                        Err(e) => {
                            error!("Failed to create file: {}", e);
                            BackendEvent::None
                        }
                    }
                },
                Err(e) => {
                    error!("Failed to serialize strategy: {}", e);
                    BackendEvent::None
                }
            }
        }
        StrategyTask::SelectStrategy(ref strategy_name) => {
            let mut selected_strategy_lock = app_state.selected_strategy.lock().await;
            let strategies_lock = app_state.available_strategies.lock().await;

            if strategies_lock.contains_key(strategy_name) {
                *selected_strategy_lock = Some(strategy_name.clone());
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::DeleteStrategy(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;
            let mut selected_strategy_lock = app_state.selected_strategy.lock().await;

            // Check if the strategy exists and remove it
            if strategies_lock.contains_key(&strategy_name) {
                strategies_lock.remove(&strategy_name);
                contract_names_lock.remove(&strategy_name);

                // If the deleted strategy was the selected one, unset the selected strategy
                if let Some(selected) = selected_strategy_lock.as_ref() {
                    if selected == &strategy_name {
                        *selected_strategy_lock = None;
                    }
                }

                BackendEvent::AppStateUpdated(AppStateUpdate::Strategies(
                    strategies_lock,
                    contract_names_lock,
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::CloneStrategy(new_strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;
            let mut selected_strategy_lock = app_state.selected_strategy.lock().await;

            if let Some(selected_strategy_name) = &*selected_strategy_lock {
                if let Some(strategy_to_clone) = strategies_lock.get(selected_strategy_name) {
                    let cloned_strategy = strategy_to_clone.clone();
                    let cloned_display_data = contract_names_lock
                        .get(selected_strategy_name)
                        .cloned()
                        .unwrap_or_default();

                    strategies_lock.insert(new_strategy_name.clone(), cloned_strategy);
                    contract_names_lock.insert(new_strategy_name.clone(), cloned_display_data);

                    *selected_strategy_lock = Some(new_strategy_name.clone());

                    BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                        new_strategy_name.clone(),
                        MutexGuard::map(strategies_lock, |strategies| {
                            strategies
                                .get_mut(&new_strategy_name)
                                .expect("strategy exists")
                        }),
                        MutexGuard::map(contract_names_lock, |names| {
                            names
                                .get_mut(&new_strategy_name)
                                .expect("inconsistent data")
                        }),
                    ))
                } else {
                    BackendEvent::None
                }
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::SetContractsWithUpdates(strategy_name, selected_contract_names) => {
            // Attain state locks
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let known_contracts_lock = app_state.known_contracts.lock().await;
            let supporting_contracts_lock = app_state.supporting_contracts.lock().await;
            let mut contract_names_lock = app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                let platform_version = PlatformVersion::latest();

                // Function to retrieve the contract from either known_contracts or
                // supporting_contracts
                let get_contract = |contract_name: &String| {
                    known_contracts_lock
                        .get(contract_name)
                        .or_else(|| supporting_contracts_lock.get(contract_name))
                        .cloned()
                };

                if let Some(first_contract_name) = selected_contract_names.first() {
                    if let Some(data_contract) = get_contract(first_contract_name) {
                        match CreatedDataContract::from_contract_and_identity_nonce(
                            data_contract,
                            u64::default(),
                            platform_version,
                        ) {
                            Ok(initial_contract) => {
                                let mut updates = BTreeMap::new();

                                for (order, contract_name) in
                                    selected_contract_names.iter().enumerate().skip(1)
                                {
                                    if let Some(update_contract) = get_contract(contract_name) {
                                        match CreatedDataContract::from_contract_and_identity_nonce(
                                            update_contract,
                                            u64::default(),
                                            platform_version,
                                        ) {
                                            Ok(created_update_contract) => {
                                                updates
                                                    .insert(order as u64, created_update_contract);
                                            }
                                            Err(e) => {
                                                error!(
                                                    "Error converting DataContract to \
                                                     CreatedDataContract for update: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                }

                                strategy.contracts_with_updates.push((
                                    initial_contract,
                                    if updates.is_empty() {
                                        None
                                    } else {
                                        Some(updates)
                                    },
                                ));
                            }
                            Err(e) => {
                                error!(
                                    "Error converting DataContract to CreatedDataContract: {:?}",
                                    e
                                );
                            }
                        }
                    }
                }

                let mut transformed_contract_names = Vec::new();
                if let Some(first_contract_name) = selected_contract_names.first() {
                    let updates: BTreeMap<u64, String> = selected_contract_names
                        .iter()
                        .enumerate()
                        .skip(1)
                        .map(|(order, name)| (order as u64, name.clone()))
                        .collect();
                    transformed_contract_names.push((first_contract_name.clone(), Some(updates)));
                }

                if let Some(existing_contracts) = contract_names_lock.get_mut(&strategy_name) {
                    existing_contracts.extend(transformed_contract_names);
                } else {
                    contract_names_lock.insert(strategy_name.clone(), transformed_contract_names);
                }

                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(contract_names_lock, |names| {
                        names.get_mut(&strategy_name).expect("inconsistent data")
                    }),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::AddOperation {
            ref strategy_name,
            ref operation,
        } => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(strategy_name) {
                strategy.operations.push(operation.clone());
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::SetIdentityInserts {
            strategy_name,
            identity_inserts_frequency,
        } => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.identities_inserts = IdentityInsertInfo {
                    frequency: identity_inserts_frequency,
                    start_keys: 3,
                    extra_keys: BTreeMap::new(),
                };
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(&strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::SetStartIdentities {
            strategy_name,
            count,
            keys_count,
            balance,
            add_transfer_key,
        } => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                let mut extra_keys = BTreeMap::new();
                if add_transfer_key {
                    extra_keys.insert(Purpose::TRANSFER,
                        [(SecurityLevel::CRITICAL, vec![KeyType::ECDSA_SECP256K1])].into());
                }
                strategy.start_identities = StartIdentities {
                    number_of_identities: count,
                    keys_per_identity: keys_count,
                    starting_balances: balance,
                    extra_keys,
                };
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(&strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::SetStartIdentitiesBalance(strategy_name, balance) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.start_identities = StartIdentities {
                    number_of_identities: strategy.start_identities.number_of_identities,
                    keys_per_identity: strategy.start_identities.keys_per_identity,
                    starting_balances: balance,
                    extra_keys: strategy.start_identities.extra_keys.clone(),
                };
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(&strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::RunStrategy(strategy_name, num_blocks, verify_proofs) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;

            // It's normal that we're asking for the mutable strategy because we need to
            // modify some properties of a contract on update
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                info!("-----Starting strategy '{}'-----", strategy_name);
                let run_start_time = Instant::now();

                let drive_lock = app_state.drive.lock().await;
                let identity_private_keys_lock = app_state.identity_private_keys.lock().await;

                // Fetch known_contracts from the chain to assure local copies match actual
                // state.
                match update_known_contracts(sdk, &app_state.known_contracts).await {
                    Ok(_) => {
                        // nothing
                    }
                    Err(e) => {
                        error!("Failed to update known contracts: {:?}", e);
                        return BackendEvent::StrategyError {
                            strategy_name: strategy_name.clone(),
                            error: format!("Failed to update known contracts: {:?}", e),
                        };
                    }
                };

                let mut loaded_identity_lock = match app_state.refresh_identity(&sdk).await {
                    Ok(lock) => lock,                
                    Err(e) => {
                        error!("Failed to refresh identity: {:?}", e);
                        return BackendEvent::StrategyError {
                            strategy_name: strategy_name.clone(),
                            error: format!("Failed to refresh identity: {:?}", e),
                        };
                    }
                };

                // Access the loaded_wallet within the Mutex
                let mut loaded_wallet_lock = app_state.loaded_wallet.lock().await;

                // Refresh UTXOs for the loaded wallet
                if let Some(ref mut wallet) = *loaded_wallet_lock {
                    let _ = wallet.reload_utxos(insight).await;
                }

                let initial_balance_identity = loaded_identity_lock.balance();
                let initial_balance_wallet = loaded_wallet_lock.clone().unwrap().balance();

                drop(loaded_wallet_lock);

                // Get block_info
                // Get block info for the first block by sending a grpc request and looking at
                // the metadata Retry up to MAX_RETRIES times
                const MAX_RETRIES: u8 = 2;
                let mut initial_block_info = BlockInfo::default();
                let mut retries = 0;
                let request = GetEpochsInfoRequest {
                    version: Some(get_epochs_info_request::Version::V0(
                        get_epochs_info_request::GetEpochsInfoRequestV0 {
                            start_epoch: None,
                            count: 1,
                            ascending: false,
                            prove: false,
                        },
                    )),
                };
                // Use retry mechanism to fetch current block info
                while retries <= MAX_RETRIES {
                    match sdk
                        .execute(request.clone(), RequestSettings::default())
                        .await
                    {
                        Ok(response) => {
                            if let Some(get_epochs_info_response::Version::V0(response_v0)) =
                                response.version
                            {
                                if let Some(metadata) = response_v0.metadata {
                                    initial_block_info = BlockInfo {
                                        time_ms: metadata.time_ms,
                                        height: metadata.height,
                                        core_height: metadata.core_chain_locked_height,
                                        epoch: Epoch::new(metadata.epoch as u16).unwrap(),
                                    };
                                }
                            }
                            break;
                        }
                        Err(e) if retries < MAX_RETRIES => {
                            error!("Error executing request, retrying: {:?}", e);
                            retries += 1;
                        }
                        Err(e) => {
                            error!("Failed to execute request after retries: {:?}", e);
                            return BackendEvent::StrategyError {
                                strategy_name: strategy_name.clone(),
                                error: format!("Failed to execute request after retries: {:?}", e),
                            };
                        }
                    }
                }
                initial_block_info.height += 1; // Add one because we'll be submitting to the next block

                // Get signer from loaded_identity
                // Convert loaded_identity to SimpleSigner
                let mut signer = {
                    let strategy_signer = strategy.signer.insert({
                        let mut new_signer = SimpleSigner::default();
                        let Identity::V0(identity_v0) = &*loaded_identity_lock;
                        for (key_id, public_key) in &identity_v0.public_keys {
                            let identity_key_tuple = (identity_v0.id, *key_id);
                            if let Some(private_key_bytes) =
                                identity_private_keys_lock.get(&identity_key_tuple)
                            {
                                new_signer
                                    .private_keys
                                    .insert(public_key.clone(), private_key_bytes.clone());
                            }
                        }
                        new_signer
                    });
                    strategy_signer.clone()
                };

                // Set initial current_identities to loaded_identity
                // On the first block of execution, start_identities will be added to current_identities
                let mut loaded_identity_clone = loaded_identity_lock.clone();
                let mut current_identities: Vec<Identity> = vec![loaded_identity_clone.clone()];

                let mut identity_nonce_counter = BTreeMap::new();
                let current_identity_nonce = sdk
                    .get_identity_nonce(
                        loaded_identity_clone.id(),
                        false,
                        Some(rs_sdk::platform::transition::put_settings::PutSettings {
                            request_settings: RequestSettings::default(),
                            identity_nonce_stale_time_s: Some(0),
                            user_fee_increase: None,
                        }))
                    .await
                    .expect("Couldn't get current identity nonce");
                identity_nonce_counter.insert(loaded_identity_clone.id(), current_identity_nonce);
                let mut contract_nonce_counter = BTreeMap::new();
                for used_contract_id in strategy.used_contract_ids() {
                    let current_identity_contract_nonce = sdk
                        .get_identity_contract_nonce(
                            loaded_identity_clone.id(),
                            used_contract_id,
                            false,
                            Some(rs_sdk::platform::transition::put_settings::PutSettings {
                                request_settings: RequestSettings::default(),
                                identity_nonce_stale_time_s: Some(0),
                                user_fee_increase: None,
                            })
                        )
                        .await
                        .expect("Couldn't get current identity contract nonce");
                    contract_nonce_counter.insert(
                        (loaded_identity_clone.id(), used_contract_id),
                        current_identity_contract_nonce,
                    );
                }

                let mut current_block_info = initial_block_info.clone();

                let mut transition_count = 0;
                let mut success_count = 0;

                while current_block_info.height < (initial_block_info.height + num_blocks) {
                    let mut document_query_callback = |query: LocalDocumentQuery| {
                        match query {
                            LocalDocumentQuery::RandomDocumentQuery(random_query) => {
                                let document_type = random_query.document_type;
                                let data_contract = random_query.data_contract;

                                // Construct a DriveQuery based on the document_type and
                                // data_contract
                                let drive_query = DriveQuery::any_item_query(
                                    data_contract,
                                    document_type.as_ref(),
                                );

                                // Query the Drive for documents
                                match drive_lock.query_documents(
                                    drive_query,
                                    None,
                                    false,
                                    None,
                                    None,
                                ) {
                                    Ok(outcome) => match outcome {
                                        QueryDocumentsOutcome::V0(outcome_v0) => {
                                            let documents = outcome_v0.documents_owned();
                                            info!(
                                                "Fetched {} documents using DriveQuery",
                                                documents.len()
                                            );
                                            documents
                                        }
                                    },
                                    Err(e) => {
                                        error!(
                                            "Block {}: Error fetching documents using DriveQuery: \
                                             {:?}",
                                            current_block_info.height, e
                                        );
                                        vec![]
                                    }
                                }
                            }
                        }
                    };
                    let mut identity_fetch_callback =
                        |identifier: Identifier, _keys_request: Option<IdentityKeysRequest>| {
                            // Convert Identifier to a byte array format expected by the Drive
                            // method
                            let identity_id_bytes = identifier.into_buffer();

                            // Fetch identity information from the Drive
                            match drive_lock.fetch_identity_with_balance(
                                identity_id_bytes,
                                None,
                                PlatformVersion::latest(),
                            ) {
                                Ok(maybe_partial_identity) => {
                                    let partial_identity =
                                        maybe_partial_identity.unwrap_or_else(|| PartialIdentity {
                                            id: identifier,
                                            loaded_public_keys: BTreeMap::new(),
                                            balance: None,
                                            revision: None,
                                            not_found_public_keys: BTreeSet::new(),
                                        });
                                    info!(
                                        "Fetched identity info for identifier {}: {:?}",
                                        identifier, partial_identity
                                    );
                                    partial_identity
                                }
                                Err(e) => {
                                    error!("Error fetching identity: {:?}", e);
                                    PartialIdentity {
                                        id: identifier,
                                        loaded_public_keys: BTreeMap::new(),
                                        balance: None,
                                        revision: None,
                                        not_found_public_keys: BTreeSet::new(),
                                    }
                                }
                            }
                        };

                    let mut create_asset_lock = {
                        let insight_ref = insight.clone();

                        move |amount: u64| -> Option<(AssetLockProof, PrivateKey)> {
                            tokio::task::block_in_place(|| {
                                let rt = tokio::runtime::Handle::current();
                                rt.block_on(async {
                                    let mut wallet_lock = app_state.loaded_wallet.lock().await;
                                    if let Some(ref mut wallet) = *wallet_lock {
                                        // Initialize old_utxos
                                        let old_utxos = match wallet {
                                            Wallet::SingleKeyWallet(ref wallet) => wallet.utxos.clone(),
                                        };

                                        // Handle asset lock transaction
                                        match wallet.asset_lock_transaction(None, amount) {
                                            Ok((asset_lock_transaction, asset_lock_proof_private_key)) => {
                                                // Use sdk_ref_clone for broadcasting and retrieving asset lock
                                                match AppState::broadcast_and_retrieve_asset_lock(&sdk, &asset_lock_transaction, &wallet.receive_address()).await {
                                                    Ok(proof) => {
                                                        // Check for new UTXOs in the wallet
                                                        let max_retries = 5;
                                                        let mut retries = 0;
                                                        let mut found_new_utxos = false;
                                                        while retries < max_retries {
                                                            let _ = wallet.reload_utxos(&insight_ref).await;
                                                            // Check if new UTXOs are available and if UTXO list is not empty
                                                            let current_utxos = match wallet {
                                                                Wallet::SingleKeyWallet(ref wallet) => &wallet.utxos,
                                                            };
                                                            if current_utxos != &old_utxos && !current_utxos.is_empty() {
                                                                found_new_utxos = true;
                                                                break;
                                                            } else {
                                                                retries += 1;
                                                                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                                                            }
                                                        }
                                                        if !found_new_utxos {
                                                            error!("Failed to find new UTXOs after maximum retries");
                                                            return None;
                                                        }
                                                        Some((proof, asset_lock_proof_private_key))
                                                    },
                                                    Err(e) => {
                                                        error!("Error broadcasting asset lock transaction: {:?}", e);
                                                        None
                                                    }
                                                }
                                            },
                                            Err(e) => {
                                                error!("Error creating asset lock transaction: {:?}", e);
                                                None
                                            }
                                        }
                                    } else {
                                        error!("Wallet not loaded");
                                        None
                                    }
                                })
                            })
                        }
                    };

                    // Get rng
                    let mut rng = StdRng::from_entropy();

                    let mut known_contracts_lock = app_state.known_contracts.lock().await;

                    // Log if you are creating start_identities, because the asset lock proofs may take a while
                    if current_block_info.height == initial_block_info.height && strategy.start_identities.number_of_identities > 0 {
                        info!(
                            "Creating {} asset lock proofs for start identities",
                            strategy.start_identities.number_of_identities
                        );
                    }

                    // Call the function to get STs for block
                    let (transitions, finalize_operations, mut new_identities) = strategy
                        .state_transitions_for_block(
                            &mut document_query_callback,
                            &mut identity_fetch_callback,
                            &mut create_asset_lock,
                            &current_block_info,
                            &mut current_identities,
                            &mut known_contracts_lock,
                            &mut signer,
                            &mut identity_nonce_counter,
                            &mut contract_nonce_counter,
                            &mut rng,
                            &StrategyConfig {
                                start_block_height: initial_block_info.height,
                                number_of_blocks: num_blocks,
                            },
                            PlatformVersion::latest(),
                        )
                        .await;

                    drop(known_contracts_lock);

                    current_identities.append(&mut new_identities);

                    // TO-DO: add documents from state transitions to explorer.drive here
                    // this is required for DocumentDelete and DocumentReplace strategy operations

                    // Process each FinalizeBlockOperation, which so far is just adding keys to the
                    // identities
                    for operation in finalize_operations {
                        match operation {
                            FinalizeBlockOperation::IdentityAddKeys(identifier, keys) => {
                                if let Some(identity) = current_identities
                                    .iter_mut()
                                    .find(|id| id.id() == identifier)
                                {
                                    for key in keys {
                                        identity.add_public_key(key);
                                    }
                                }
                            }
                        }
                    }

                    // Update the loaded_identity_clone and loaded_identity_lock with the latest
                    // state of the identity
                    if let Some(modified_identity) = current_identities
                        .iter()
                        .find(|identity| identity.id() == loaded_identity_clone.id())
                    {
                        loaded_identity_clone = modified_identity.clone();
                        *loaded_identity_lock = modified_identity.clone();
                    }

                    let mut st_queue = VecDeque::new();

                    // Put the state transitions in the queue if not empty
                    if !transitions.is_empty() {
                        st_queue = transitions.into();
                        info!(
                            "Prepared {} state transitions for block {}",
                            st_queue.len(),
                            current_block_info.height
                        );
                    } else {
                        // Log when no state transitions are found for a block
                        info!(
                            "No state transitions prepared for block {}",
                            current_block_info.height
                        );
                    }

                    if st_queue.is_empty() {
                        info!(
                            "No state transitions to process for block {}",
                            current_block_info.height
                        );
                    } else {
                        let mut st_queue_index = 0;
                        let mut broadcast_futures = Vec::new();

                        for transition in st_queue.iter() {
                            // Init
                            transition_count += 1;
                            st_queue_index += 1;
                            let transition_clone = transition.clone();
                            let transition_type = transition_clone.name().to_owned();

                            // Dependent state transitions are those that get their revision checked. Sending multiple
                            // in the same block causes errors because they get sent to different nodes and become disordered.
                            // So we sleep for 1 second for dependent transitions so that they only go 1 per block.
                            let is_dependent_transition = matches!(
                                transition_clone,
                                StateTransition::IdentityUpdate(_)
                                    | StateTransition::DataContractUpdate(_)
                                    | StateTransition::IdentityCreditTransfer(_)
                                    | StateTransition::IdentityCreditWithdrawal(_)
                            );

                            if is_dependent_transition {
                                // Sequentially process dependent transitions with a delay between
                                // them
                                if let Ok(broadcast_request) =
                                    transition_clone.broadcast_request_for_state_transition()
                                {
                                    match broadcast_request
                                        .execute(sdk, RequestSettings::default())
                                        .await
                                    {
                                        Ok(_broadcast_result) => {
                                            if let Ok(wait_request) = transition_clone
                                                .wait_for_state_transition_result_request()
                                            {
                                                match wait_request
                                                    .execute(sdk, RequestSettings::default())
                                                    .await
                                                {
                                                    Ok(wait_response) => {
                                                        if let Some(wait_for_state_transition_result_response::Version::V0(v0_response)) = &wait_response.version {
                                                            if let Some(metadata) = &v0_response.metadata {
                                                                if !verify_proofs {
                                                                    success_count += 1;
                                                                    info!("Successfully processed state transition {} ({}) for block {} (Actual block height: {})", st_queue_index, transition_type, current_block_info.height, metadata.height);
                                                                }
                                                                // Additional logging to inspect the result regardless of metadata presence
                                                                match &v0_response.result {
                                                                    Some(wait_for_state_transition_result_response_v0::Result::Error(error)) => {
                                                                        error!("WaitForStateTransitionResultResponse error: {:?}", error);
                                                                    }
                                                                    Some(wait_for_state_transition_result_response_v0::Result::Proof(proof)) => {
                                                                        if verify_proofs {
                                                                            let epoch = Epoch::new(metadata.epoch as u16).expect("Expected to get epoch from metadata in proof verification");
                                                                            let verified = Drive::verify_state_transition_was_executed_with_proof(
                                                                                &transition_clone,
                                                                                &BlockInfo {
                                                                                    time_ms: metadata.time_ms,
                                                                                    height: metadata.height,
                                                                                    core_height: metadata.core_chain_locked_height,
                                                                                    epoch,
                                                                                },
                                                                                proof.grovedb_proof.as_slice(),
                                                                                &|_| Ok(None),
                                                                                sdk.version(),
                                                                            );
                                                                            match verified {
                                                                                Ok(_) => {
                                                                                    success_count += 1;
                                                                                    info!("Successfully processed and verified proof for state transition {} ({}), block {} (Actual block height: {})", st_queue_index, transition_type, current_block_info.height, metadata.height);
                                                                                }
                                                                                Err(e) => {
                                                                                    error!("Error verifying state transition execution proof: {}", e);
                                                                                }
                                                                            }    
                                                                        }
                                                                    }
                                                                    _ => {}
                                                                }
                                                            } else {
                                                                if let Some(result) = &v0_response.result {
                                                                    match result {
                                                                        wait_for_state_transition_result_response_v0::Result::Error(e) => tracing::error!("{:?}", e),
                                                                        wait_for_state_transition_result_response_v0::Result::Proof(_) => tracing::info!("Proof received but no metadata present so we can't verify it."),
                                                                    }
                                                                }
                                                            }
                                                          
                                                            // Sleep because we need to give the chain state time to update revisions
                                                            // It seems this is only necessary for certain STs. Like AddKeys and DisableKeys seem to need it, but Transfer does not. Not sure about Withdraw or ContractUpdate yet.
                                                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                                        } else {
                                                            info!("Response version other than V0 received or absent for state transition {} ({})", st_queue_index, transition_type);
                                                        }
                                                    }
                                                    Err(e) => error!(
                                                        "Error waiting for state transition result: {:?}",
                                                        e
                                                    ),
                                                }
                                            } else {
                                                error!(
                                                    "Failed to create wait request for state transition."
                                                );
                                            }
                                        }
                                        Err(e) => error!(
                                            "Error broadcasting dependent state transition: {:?}",
                                            e
                                        ),
                                    }
                                } else {
                                    error!(
                                        "Failed to create broadcast request for state transition."
                                    );
                                }
                            } else {
                                // Prepare futures for broadcasting independent transitions
                                let future = async move {
                                    match transition_clone.broadcast_request_for_state_transition()
                                    {
                                        Ok(broadcast_request) => {
                                            let broadcast_result = broadcast_request
                                                .execute(sdk, RequestSettings::default())
                                                .await;
                                            Ok((transition_clone, broadcast_result))
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "Error preparing broadcast request for state transition {} block height {}: {:?}",
                                                st_queue_index,
                                                current_block_info.height,
                                                e
                                            );
                                            Err(e)
                                        },
                                    }
                                };
                                broadcast_futures.push(future);
                            }
                        }

                        // Concurrently execute all broadcast requests for independent transitions
                        let broadcast_results = join_all(broadcast_futures).await;

                        // Prepare futures for waiting for state transition results
                        let mut wait_futures = Vec::new();
                        for (index, result) in broadcast_results.into_iter().enumerate() {
                            match result {
                                Ok((transition, broadcast_result)) => {
                                    let transition_type = transition.name().to_owned();

                                    if broadcast_result.is_err() {
                                        error!(
                                            "Error broadcasting state transition {} ({}) for block height {}: {:?}",
                                            index + 1,
                                            transition_type,
                                            current_block_info.height,
                                            broadcast_result.err().unwrap()
                                        );
                                        continue;
                                    }

                                    // Extract the data contract ID from the transition
                                    let data_contract_id_option = match &transition {
                                        StateTransition::DocumentsBatch(DocumentsBatchTransition::V0(documents_batch)) => {
                                            documents_batch.transitions.get(0).and_then(|document_transition| {
                                                match document_transition {
                                                    DocumentTransition::Create(DocumentCreateTransition::V0(create_transition)) => {
                                                        Some(create_transition.base.data_contract_id())
                                                    },
                                                    // Add handling for Replace and Delete transitions if necessary
                                                    _ => None,
                                                }
                                            })
                                        },
                                        _ => None,
                                    };

                                    let known_contracts_lock = app_state.known_contracts.lock().await;

                                    let data_contract_clone = if let Some(data_contract_id) = data_contract_id_option {
                                        let data_contract_id_str = data_contract_id.to_string(Encoding::Base58);
                                        known_contracts_lock.get(&data_contract_id_str).cloned()
                                    } else {
                                        None
                                    };

                                    drop(known_contracts_lock);

                                    let wait_future = async move {
                                        let wait_result = match transition.wait_for_state_transition_result_request() {
                                            Ok(wait_request) => wait_request.execute(sdk, RequestSettings::default()).await,
                                            Err(e) => {
                                                error!(
                                                    "Error creating wait request for state transition {} block height {}: {:?}",
                                                    index + 1, current_block_info.height, e
                                                );
                                                return None;
                                            }
                                        };

                                        match wait_result {
                                            Ok(wait_response) => {
                                                Some(if let Some(wait_for_state_transition_result_response::Version::V0(v0_response)) = &wait_response.version {
                                                    if let Some(metadata) = &v0_response.metadata {
                                                        if !verify_proofs {
                                                            info!(
                                                                "Successfully processed state transition {} ({}) for block {} (Actual block height: {})",
                                                                index + 1, transition.name(), current_block_info.height, metadata.height
                                                            );    
                                                        }

                                                        // Verification of the proof
                                                        if let Some(wait_for_state_transition_result_response_v0::Result::Proof(proof)) = &v0_response.result {
                                                            if verify_proofs {
                                                                let epoch = Epoch::new(metadata.epoch as u16).expect("Expected to get epoch from metadata in proof verification");
                                                                // For proof verification, if it's a DocumentsBatch, include the data contract, else don't
                                                                let verified = if transition.name() == "DocumentsBatch" {
                                                                    match data_contract_clone.as_ref() {
                                                                        Some(data_contract) => {
                                                                            let epoch = Epoch::new(metadata.epoch as u16).expect("Expected to get epoch from metadata in proof verification");
                                                                            Drive::verify_state_transition_was_executed_with_proof(
                                                                                &transition,
                                                                                &BlockInfo {
                                                                                    time_ms: metadata.time_ms,
                                                                                    height: metadata.height,
                                                                                    core_height: metadata.core_chain_locked_height,
                                                                                    epoch,
                                                                                },
                                                                                proof.grovedb_proof.as_slice(),
                                                                                &|_| Ok(Some(data_contract.clone().into())),
                                                                                sdk.version(),
                                                                            )
                                                                        }
                                                                        None => Err(drive::error::Error::Proof(ProofError::UnknownContract("Data contract ID not found in known_contracts".into()))),
                                                                    }
                                                                } else {
                                                                    let epoch = Epoch::new(metadata.epoch as u16).expect("Expected to get epoch from metadata in proof verification");
                                                                    Drive::verify_state_transition_was_executed_with_proof(
                                                                        &transition,
                                                                        &BlockInfo {
                                                                            time_ms: metadata.time_ms,
                                                                            height: metadata.height,
                                                                            core_height: metadata.core_chain_locked_height,
                                                                            epoch,
                                                                        },
                                                                        proof.grovedb_proof.as_slice(),
                                                                        &|_| Ok(None),
                                                                        sdk.version(),
                                                                    )
                                                                };

                                                                match verified {
                                                                    Ok(_) => {
                                                                        info!("Successfully processed and verified proof for state transition {} ({}), block {} (Actual block height: {})", index + 1, transition_type, current_block_info.height, metadata.height);
                                                                    }
                                                                    Err(e) => error!("Error verifying state transition execution proof: {}", e),
                                                                }
                                                            }

                                                            // If it's the first block, sleep for one second so we don't, for example, register a doc to a new contract
                                                            if current_block_info.height == initial_block_info.height {
                                                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                                            }
                                                        }
                                                        
                                                        // If a data contract was registered, add it to
                                                        // known_contracts
                                                        if let StateTransition::DataContractCreate(
                                                            DataContractCreateTransition::V0(
                                                                data_contract_create_transition,
                                                            ),
                                                        ) = &transition
                                                        {
                                                            // Extract the data contract from the transition
                                                            let data_contract_serialized =
                                                                &data_contract_create_transition
                                                                    .data_contract;
                                                            let data_contract_result =
                                                                DataContract::try_from_platform_versioned(
                                                                    data_contract_serialized.clone(),
                                                                    false,
                                                                    PlatformVersion::latest(),
                                                                );

                                                            match data_contract_result {
                                                                Ok(data_contract) => {
                                                                    let mut known_contracts_lock =
                                                                        app_state
                                                                            .known_contracts
                                                                            .lock()
                                                                            .await;
                                                                    known_contracts_lock.insert(
                                                                        data_contract
                                                                            .id()
                                                                            .to_string(Encoding::Base58),
                                                                        data_contract,
                                                                    );
                                                                }
                                                                Err(e) => {
                                                                    error!(
                                                                        "Error deserializing data \
                                                                        contract: {:?}",
                                                                        e
                                                                    );
                                                                }
                                                            }
                                                        }
                                                        
                                                        // Log the Base58 encoded IDs of any created Contracts
                                                        match transition.clone() {
                                                            StateTransition::DataContractCreate(contract_create_transition) => {
                                                                let ids = contract_create_transition.modified_data_ids();
                                                                for id in ids {
                                                                    let encoded_id: String = id.into();
                                                                    info!("Created Contract: {}", encoded_id);
                                                                }
                                                            },
                                                            _ => {
                                                                // nothing
                                                            }
                                                        }

                                                        // Log the Base58 encoded IDs of any created Identities
                                                        match transition.clone() {
                                                            StateTransition::IdentityCreate(identity_create_transition) => {
                                                                let ids = identity_create_transition.modified_data_ids();
                                                                for id in ids {
                                                                    let encoded_id: String = id.into();
                                                                    info!("Created Identity: {}", encoded_id);
                                                                }
                                                            },
                                                            _ => {
                                                                // nothing
                                                            }
                                                        }
                                                    } else {
                                                        if let Some(result) = &v0_response.result {
                                                            match result {
                                                                wait_for_state_transition_result_response_v0::Result::Error(e) => tracing::error!("{:?}", e),
                                                                wait_for_state_transition_result_response_v0::Result::Proof(_) => tracing::info!("Proof received but no metadata present so we can't verify it."),
                                                            }
                                                        }

                                                        // Log the Base58 encoded IDs of any created Identities
                                                        match transition.clone() {
                                                            StateTransition::IdentityCreate(identity_create_transition) => {
                                                                let ids = identity_create_transition.modified_data_ids();
                                                                for id in ids {
                                                                    let encoded_id: String = id.into();
                                                                    info!("Created Identity: {}", encoded_id);
                                                                }
                                                            },
                                                            _ => {
                                                                // nothing
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    tracing::error!("wait_response.version is None");
                                                })
                                            },
                                            Err(e) => {
                                                error!("Wait result error: {:?}", e);
                                                None
                                            }
                                        }
                                    };
                                    wait_futures.push(wait_future);
                                },
                                Err(e) => {
                                    error!(
                                        "Error preparing broadcast request for state transition {} block height {}: {:?}",
                                        index + 1,
                                        current_block_info.height,
                                        e
                                    );
                                }
                            }
                        }

                        // Wait for all state transition result futures to complete
                        let wait_results = join_all(wait_futures).await;

                        // Log the actual block height for each state transition
                        for (_, actual_block_height) in wait_results.into_iter().enumerate() {
                            match actual_block_height {
                                Some(_) => {
                                    success_count += 1;
                                }
                                None => continue,
                            }
                        }
                    }

                    // Update block_info
                    current_block_info.height += 1;
                    current_block_info.time_ms += 1 * 1000; // plus 1 second
                }

                info!("-----Strategy '{}' finished running-----", strategy_name);
                let run_time = run_start_time.elapsed();

                // Refresh the identity at the end
                drop(loaded_identity_lock);
                let refresh_result = app_state.refresh_identity(&sdk).await;
                if let Err(ref e) = refresh_result {
                    error!("Failed to refresh identity after running strategy: {:?}", e);
                }

                // Attempt to retrieve the final balance from the refreshed identity
                let final_balance_identity = match refresh_result {
                    Ok(refreshed_identity_lock) => {
                        // Successfully refreshed, now access the balance
                        refreshed_identity_lock.balance()
                    }
                    Err(_) => {
                        error!("Error refreshing identity after running strategy");
                        initial_balance_identity
                    }
                };

                let dash_spent_identity = (initial_balance_identity as f64
                    - final_balance_identity as f64)
                    / 100_000_000_000.0;

                let loaded_wallet_lock = app_state.loaded_wallet.lock().await;
                let final_balance_wallet = loaded_wallet_lock.clone().unwrap().balance();
                let dash_spent_wallet = (initial_balance_wallet as f64
                    - final_balance_wallet as f64)
                    / 100_000_000_000.0;

                info!(
                    "-----Strategy '{}' completed-----\n\nState transitions attempted: {}\nState \
                    transitions succeeded: {}\nNumber of blocks: {}\nRun time: \
                    {:?}\nDash spent (Loaded Identity): {}\nDash spent (Wallet): {}\n",
                    strategy_name,
                    transition_count,
                    success_count,
                    (current_block_info.height - initial_block_info.height),
                    run_time,
                    dash_spent_identity,
                    dash_spent_wallet,
                );

                BackendEvent::StrategyCompleted {
                    strategy_name: strategy_name.clone(),
                    result: StrategyCompletionResult::Success {
                        final_block_height: current_block_info.height,
                        start_block_height: initial_block_info.height,
                        success_count,
                        transition_count,
                        run_time,
                        dash_spent_identity,
                        dash_spent_wallet,
                    },
                }
            } else {
                tracing::error!("No strategy loaded with name \"{}\"", strategy_name);
                BackendEvent::None
            }
        }
        StrategyTask::RemoveLastContract(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                // Remove the last contract_with_update entry from the strategy
                strategy.contracts_with_updates.pop();

                // Also remove the corresponding entry from the displayed contracts
                if let Some(contract_names) = contract_names_lock.get_mut(&strategy_name) {
                    // Assuming each entry in contract_names corresponds to an entry in
                    // contracts_with_updates
                    contract_names.pop();
                }

                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(contract_names_lock, |names| {
                        names.get_mut(&strategy_name).expect("inconsistent data")
                    }),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::RemoveIdentityInserts(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.identities_inserts = IdentityInsertInfo::default();
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(&strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::RemoveStartIdentities(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.start_identities = StartIdentities::default();
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(&strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
        StrategyTask::RemoveLastOperation(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.operations.pop();
                BackendEvent::AppStateUpdated(AppStateUpdate::SelectedStrategy(
                    strategy_name.clone(),
                    MutexGuard::map(strategies_lock, |strategies| {
                        strategies.get_mut(&strategy_name).expect("strategy exists")
                    }),
                    MutexGuard::map(
                        app_state.available_strategies_contract_names.lock().await,
                        |names| names.get_mut(&strategy_name).expect("inconsistent data"),
                    ),
                ))
            } else {
                BackendEvent::None
            }
        }
    }
}

pub async fn update_known_contracts(
    sdk: &Sdk,
    known_contracts: &Mutex<KnownContractsMap>,
) -> Result<(), String> {
    let contract_ids = {
        let contracts_lock = known_contracts.lock().await;
        contracts_lock.keys().cloned().collect::<Vec<String>>()
    };

    // Clear known contracts first. This is necessary for some reason, I don't know.
    let mut contracts_lock = known_contracts.lock().await;
    contracts_lock.clear();
    drop(contracts_lock);

    for contract_id_str in contract_ids.iter() {
        let contract_id = Identifier::from_string(contract_id_str, Encoding::Base58)
            .expect("Failed to convert ID string to Identifier");

        match DataContract::fetch(&*sdk, contract_id).await {
            Ok(Some(data_contract)) => {
                let mut contracts_lock = known_contracts.lock().await;
                contracts_lock.insert(contract_id_str.clone(), data_contract);
            }
            Ok(None) => {
                error!("Contract not found for ID: {}", contract_id_str);
            }
            Err(e) => {
                return Err(format!(
                    "Error fetching contract {}: {}",
                    contract_id_str, e
                ));
            }
        }
    }

    Ok(())
}
