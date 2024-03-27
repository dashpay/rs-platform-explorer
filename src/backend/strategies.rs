//! Strategies management backend module.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque}, fs::File, io::Write, sync::{atomic::{AtomicUsize, Ordering}, Arc}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}
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
        accessors::v0::{DataContractV0Getters, DataContractV0Setters}, created_data_contract::CreatedDataContract, DataContract
    }, identity::{
        accessors::IdentityGettersV0, state_transition::asset_lock_proof::AssetLockProof, Identity,
        PartialIdentity,
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
    frequency::Frequency, operations::{FinalizeBlockOperation, Operation}, IdentityInsertInfo, LocalDocumentQuery, StartIdentities, Strategy, StrategyConfig
};
use tokio::sync::{Mutex, MutexGuard};
use tracing::{error, info};

use super::{
    insight::InsightAPIClient, state::{ContractFileName, KnownContractsMap}, AppState, AppStateUpdate,
    BackendEvent, StrategyCompletionResult, StrategyContractNames,
};
use crate::backend::Wallet;

#[derive(Debug, PartialEq, Clone)]
pub(crate) enum StrategyTask {
    CreateStrategy(String),
    ImportStrategy(String),
    ExportStrategy(String),
    SelectStrategy(String),
    DeleteStrategy(String),
    CloneStrategy(String),
    SetContractsWithUpdates(String, Vec<String>),
    SetContractsWithUpdatesRandom(String, String, u8),
    SetIdentityInserts {
        strategy_name: String,
        identity_inserts_frequency: Frequency,
    },
    SetStartIdentities {
        strategy_name: String,
        count: u8,
        keys_count: u8,
        balance: u64,
    },
    SetStartIdentitiesBalance(String, u64),
    AddOperation {
        strategy_name: String,
        operation: Operation,
    },
    RunStrategy(String, u64, bool, bool),
    RemoveLastContract(String),
    ClearContracts(String),
    RemoveIdentityInserts(String),
    RemoveStartIdentities(String),
    RemoveLastOperation(String),
}

pub(crate) async fn run_strategy_task<'s>(
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
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let known_contracts_lock = app_state.known_contracts.lock().await;
            let supporting_contracts_lock = app_state.supporting_contracts.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

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

                // Get the loaded identity nonce
                let loaded_identity_lock = match app_state.refresh_identity(&sdk).await {
                    Ok(lock) => lock,
                    Err(e) => {
                        error!("Failed to refresh identity: {:?}", e);
                        return BackendEvent::StrategyError {
                            strategy_name: strategy_name.clone(),
                            error: format!("Failed to refresh identity: {:?}", e),
                        };
                    }
                };
                let identity_nonce = sdk
                    .get_identity_nonce(loaded_identity_lock.id(), true, None)
                    .await
                    .expect("Couldn't get current identity nonce");

                if let Some(first_contract_name) = selected_contract_names.first() {
                    if let Some(data_contract) = get_contract(first_contract_name) {
                        match CreatedDataContract::from_contract_and_identity_nonce(
                            data_contract,
                            identity_nonce,
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
                                            identity_nonce,
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
        StrategyTask::SetContractsWithUpdatesRandom(strategy_name, selected_contract_name, variants) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let known_contracts_lock = app_state.known_contracts.lock().await;
            let mut supporting_contracts_lock = app_state.supporting_contracts.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

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

                // Get the loaded identity nonce
                let loaded_identity_lock = match app_state.refresh_identity(&sdk).await {
                    Ok(lock) => lock,
                    Err(e) => {
                        error!("Failed to refresh identity: {:?}", e);
                        return BackendEvent::StrategyError {
                            strategy_name: strategy_name.clone(),
                            error: format!("Failed to refresh identity: {:?}", e),
                        };
                    }
                };
                let mut identity_nonce = sdk
                    .get_identity_nonce(loaded_identity_lock.id(), true, None)
                    .await
                    .expect("Couldn't get current identity nonce");

                // Add the contracts to the strategy contracts_with_updates
                if let Some(data_contract) = get_contract(&selected_contract_name) {
                    match CreatedDataContract::from_contract_and_identity_nonce(
                        data_contract,
                        identity_nonce,
                        platform_version,
                    ) {
                        Ok(original_contract) => {
                            // Add original contract to the strategy
                            let mut contract_variants: Vec<CreatedDataContract> = Vec::new();
                            contract_variants.push(original_contract.clone());
                            strategy.contracts_with_updates.push((original_contract.clone(), None));
                            
                            // Add i variants of the original contract to the strategy
                            for i in 0..variants-1 {
                                let mut new_data_contract = original_contract.data_contract().clone();
                                let new_id = DataContract::generate_data_contract_id_v0(loaded_identity_lock.id(), identity_nonce);
                                new_data_contract.set_id(new_id);
                                match CreatedDataContract::from_contract_and_identity_nonce(
                                    new_data_contract.clone(),
                                    identity_nonce,
                                    platform_version,
                                ) {
                                    Ok(contract) => {
                                        contract_variants.push(contract.clone());
                                        strategy.contracts_with_updates.push((contract, None));
                                        let new_contract_name = String::from(format!("{}_variant_{}", selected_contract_name, i));
                                        supporting_contracts_lock.insert(new_contract_name, new_data_contract);
                                        identity_nonce += 1;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Error converting DataContract to CreatedDataContract variant: {:?}",
                                            e
                                        );            
                                    }
                                };
                            }

                            let contract_id_strings: Vec<(String, Option<BTreeMap<u64, String>>)> = contract_variants.iter().map(|x| (x.data_contract().id().to_string(Encoding::Base58), None)).collect();
                            
                            // Add the new contracts to app_state.available_strategies_contract_names
                            if let Some(existing_strategy_contracts) = contract_names_lock.get_mut(&strategy_name) {
                                existing_strategy_contracts.extend(contract_id_strings);
                            } else {
                                contract_names_lock.insert(strategy_name.clone(), contract_id_strings);
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "Error converting original DataContract to CreatedDataContract: {:?}",
                                e
                            );
                        }
                    }
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
        } => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.start_identities = StartIdentities {
                    number_of_identities: count,
                    keys_per_identity: keys_count,
                    starting_balances: balance,
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
        StrategyTask::RunStrategy(strategy_name, num_blocks_or_seconds, verify_proofs, block_mode) => {

            tracing::info!("-----Starting strategy '{}'-----", strategy_name);
            
            // Fetch known_contracts from the chain to assure local copies match actual
            // state.
            match update_known_contracts(sdk, &app_state.known_contracts).await {
                Ok(_) => {
                    // nothing
                },
                Err(e) => {
                    error!("Failed to update known contracts: {:?}", e);
                    return BackendEvent::StrategyError {
                        strategy_name: strategy_name.clone(),
                        error: format!("Failed to update known contracts: {:?}", e),
                    };
                }
            };

            // Refresh loaded_identity and get the current balance at strategy start
            let mut loaded_identity_lock = match app_state.refresh_identity(&sdk).await {
                Ok(lock) => {
                    lock
                },                
                Err(e) => {
                    error!("Failed to refresh identity: {:?}", e);
                    return BackendEvent::StrategyError {
                        strategy_name: strategy_name.clone(),
                        error: format!("Failed to refresh identity: {:?}", e),
                    };
                }
            };
            let initial_balance_identity = loaded_identity_lock.balance();

            // Refresh UTXOs for the loaded wallet and get initial wallet balance
            let mut loaded_wallet_lock = app_state.loaded_wallet.lock().await;
            if let Some(ref mut wallet) = *loaded_wallet_lock {
                let _ = wallet.reload_utxos(insight).await;
            }
            let initial_balance_wallet = loaded_wallet_lock.clone().unwrap().balance();
            drop(loaded_wallet_lock);

            // Get a mutable strategy because we need to modify some properties of contracts on updates
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
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
                let identity_private_keys_lock = app_state.identity_private_keys.lock().await;
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
                drop(identity_private_keys_lock);

                // Set initial current_identities to loaded_identity
                // During strategy execution, newly created identities will be added to current_identities
                let mut loaded_identity_clone = loaded_identity_lock.clone();
                let mut current_identities: Vec<Identity> = vec![loaded_identity_clone.clone()];

                // Set the nonce counters
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

                // Get a lock on the local drive for the following two callbacks
                let drive_lock = app_state.drive.lock().await;

                // Callback used to fetch documents from the local Drive instance
                // Used for DocumentReplace and DocumentDelete transitions
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
                                    error!("Error fetching documents using DriveQuery: {:?}", e);
                                    vec![]
                                }
                            }
                        }
                    }
                };

                // Callback used to fetch identities from the local Drive instance
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

                // Callback used for creating asset locks for identity create and top up transitions
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

                // Some final initialization
                let mut rng = StdRng::from_entropy(); // Will be passed to state_transitions_for_block
                let mut current_block_info = initial_block_info.clone(); // Used for transition creation and logging
                let mut transition_count = 0; // Used for logging how many transitions we attempted
                let mut success_count = 0; // Used for logging how many transitions were successful
                let execution_start_time = Instant::now(); // Time when actual execution begins
                let mut index = 1; // Index of the loop iteration. Represents blocks for block mode and seconds for time mode
                let oks = Arc::new(AtomicUsize::new(0)); // Atomic counter for successful broadcasts
                let errs = Arc::new(AtomicUsize::new(0)); // Atomic counter for failed broadcasts

                // Now loop through the number of blocks or seconds the user asked for, preparing and processing state transitions
                while (block_mode && current_block_info.height < (initial_block_info.height + num_blocks_or_seconds))
                    || (!block_mode && execution_start_time.elapsed().as_secs() < num_blocks_or_seconds) {
                    
                    // Get the execution mode as a string for logging
                    let mut mode_string = String::new();
                    if block_mode {
                        mode_string.push_str("block");
                    } else {
                        mode_string.push_str("second");
                    }

                    let oks_clone = oks.clone();
                    let errs_clone = errs.clone();
                    let loop_start_time = Instant::now();

                    // Log if you are creating start_identities, because the asset lock proofs take a while
                    if current_block_info.height == initial_block_info.height && strategy.start_identities.number_of_identities > 0 {
                        info!(
                            "Creating {} asset lock proofs for start identities...",
                            strategy.start_identities.number_of_identities
                        );
                    }

                    // Need to pass app_state.known_contracts to state_transitions_for_block
                    let mut known_contracts_lock = app_state.known_contracts.lock().await;

                    // Get the state transitions for the block (or second)
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
                                number_of_blocks: num_blocks_or_seconds,
                            },
                            PlatformVersion::latest(),
                        )
                        .await;

                    drop(known_contracts_lock);

                    // Add the identities that will be created to current_identities.
                    // TO-DO: This should be moved to execution after we confirm they were registered.
                    current_identities.append(&mut new_identities);

                    // TO-DO: for DocumentDelete and DocumentReplace strategy operations, we need to 
                    // add documents from state transitions to the local Drive instance here

                    // Process each FinalizeBlockOperation, which so far is just adding keys to identities
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

                    // Update the loaded_identity_clone and loaded_identity_lock with the latest state of the identity
                    if let Some(modified_identity) = current_identities
                        .iter()
                        .find(|identity| identity.id() == loaded_identity_clone.id())
                    {
                        loaded_identity_clone = modified_identity.clone();
                        *loaded_identity_lock = modified_identity.clone();
                    }

                    // Now process the state transitions
                    if !transitions.is_empty() {
                        tracing::info!(
                            "Prepared {} state transitions for {} {}",
                            transitions.len(),
                            mode_string,
                            index
                        );

                        // A queue for the state transitions for the block (or second)
                        let st_queue: VecDeque<StateTransition> = transitions.clone().into();
                        let mut st_queue_index = 0; // Current index of the queue. Only used for logging.
                        
                        // We will concurrently broadcast the state transitions, so collect the futures
                        let mut broadcast_futures = Vec::new();

                        for transition in st_queue.iter() {
                            transition_count += 1; // Used for logging how many transitions we attempted
                            st_queue_index += 1; // Start at 1 and iterate upwards since we're only using this for logs
                            let transition_clone = transition.clone();
                            let transition_type = transition_clone.name().to_owned();
                        
                            // Determine if the transitions is a dependent transition.
                            // Dependent state transitions are those that get their revision checked. Sending multiple
                            // in the same block causes errors because they get sent to different nodes and become disordered.
                            // So we sleep for 1 second between dependent transitions to enforce only 1 per block.
                            let is_dependent_transition = matches!(
                                transition_clone,
                                StateTransition::IdentityUpdate(_)
                                    | StateTransition::DataContractUpdate(_)
                                    | StateTransition::IdentityCreditTransfer(_)
                                    | StateTransition::IdentityCreditWithdrawal(_)
                            );

                            if is_dependent_transition {
                                // Sequentially process dependent transitions with a delay between them
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
                                                                success_count += 1;
                                                                if !verify_proofs {
                                                                    info!("Successfully processed state transition {} ({}) for {} {} (Actual block height: {})", st_queue_index, transition_type, mode_string, index, metadata.height);
                                                                }
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
                                                                                    info!("Successfully processed and verified proof for state transition {} ({}), {} {} (Actual block height: {})", st_queue_index, transition_type, mode_string, index, metadata.height);
                                                                                }
                                                                                Err(e) => {
                                                                                    error!("Error verifying state transition execution proof: {}", e);
                                                                                }
                                                                            }    
                                                                        }
                                                                    }
                                                                    _ => {}
                                                                }

                                                                // Sleep because we need to give the chain state time to update revisions
                                                                // It seems this is only necessary for certain STs. Like AddKeys and DisableKeys seem to need it, but Transfer does not. Not sure about Withdraw or ContractUpdate yet.
                                                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                                            }
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
                                let oks = oks_clone.clone();
                                let errs = errs_clone.clone();

                                // Prepare futures for broadcasting independent transitions
                                let future = async move {                            
                                    match transition_clone.broadcast_request_for_state_transition() {
                                        Ok(broadcast_request) => {
                                            let broadcast_result = broadcast_request.execute(sdk, RequestSettings::default()).await;
                                            match broadcast_result {
                                                Ok(_) => {
                                                    oks.fetch_add(1, Ordering::SeqCst);
                                                    if !block_mode {
                                                        tracing::info!("Successfully broadcasted transition: {}", transition_clone.name());
                                                    }
                                                    Ok((transition_clone, broadcast_result))
                                                },
                                                Err(e) => {
                                                    errs.fetch_add(1, Ordering::SeqCst);
                                                    tracing::error!("Failed to broadcast transition: {}, Error: {:?}", transition_clone.name(), e);
                                                    Err(e)
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            errs.fetch_add(1, Ordering::SeqCst);
                                            tracing::error!("Error preparing broadcast for transition: {}, Error: {:?}", transition_clone.name(), e);
                                            Err(e)
                                        }.expect("Expected to prepare broadcast for request for state transition") // I guess I have to do this to make it compile
                                    }
                                };
                                broadcast_futures.push(future);
                            }
                        }

                        // Concurrently execute all broadcast requests for independent transitions
                        let broadcast_results = join_all(broadcast_futures).await;

                        // If we're in block mode, we're going to wait for state transition results and potentially verify proofs too.
                        // If we're in time mode, we're just broadcasting.
                        if block_mode {
                            let mut wait_futures = Vec::new();
                            for (index, result) in broadcast_results.into_iter().enumerate() {
                                match result {
                                    Ok((transition, broadcast_result)) => {
                                        let transition_type = transition.name().to_owned();
    
                                        if broadcast_result.is_err() {
                                            error!(
                                                "Error broadcasting state transition {} ({}) for {} {}: {:?}",
                                                index + 1,
                                                transition_type,
                                                mode_string,
                                                index,
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
                                            // Handle other state transition types that involve data contracts here
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
                                            let mut mode_string = String::new();
                                            if block_mode {
                                                mode_string.push_str("block");
                                            } else {
                                                mode_string.push_str("second");
                                            }
                                            let wait_result = match transition.wait_for_state_transition_result_request() {
                                                Ok(wait_request) => wait_request.execute(sdk, RequestSettings::default()).await,
                                                Err(e) => {
                                                    error!(
                                                        "Error creating wait request for state transition {} {} {}: {:?}",
                                                        index + 1, mode_string, index, e
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
                                                                    "Successfully processed state transition {} ({}) for {} {} (Actual block height: {})",
                                                                    index + 1, transition.name(), mode_string, index, metadata.height
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
                                                                            info!("Successfully processed and verified proof for state transition {} ({}), {} {} (Actual block height: {})", index + 1, transition_type, mode_string, index, metadata.height);
                                                                            
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
                                                                        }
                                                                        Err(e) => error!("Error verifying state transition execution proof: {}", e),
                                                                    }
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
                                            "Error preparing broadcast request for state transition {} {} {}: {:?}",
                                            index + 1,
                                            mode_string,
                                            index,
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
                        } else {
                            // Time mode
                            // Sleep for one second on first block to make sure we don't submit documents or updates in the same block as contract or identity creation
                            if index == 1 {
                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            }
                        }
                    } else {
                        // No transitions prepared for the block (or second)
                        tracing::info!(
                            "No state transitions prepared for {} {}",
                            mode_string,
                            current_block_info.height
                        );
                    }

                    // Update current_block_info and index for next loop iteration
                    current_block_info.height += 1;
                    let current_time_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("time went backwards")
                        .as_millis();
                    current_block_info.time_ms = current_time_ms as u64;
                    index += 1;

                    // Make sure the loop doesn't iterate faster than once per second in time mode
                    if !block_mode {
                        let elapsed = loop_start_time.elapsed();
                        if elapsed < Duration::from_secs(1) {
                            let remaining_time = Duration::from_secs(1) - elapsed;
                            tokio::time::sleep(remaining_time).await;
                        }    
                    }
                }

                // Strategy execution is finished
                tracing::info!("-----Strategy '{}' finished running-----", strategy_name);

                // Log oks and errs
                tracing::info!("Successfully processed: {}, Failed to process: {}", oks.load(Ordering::SeqCst), errs.load(Ordering::SeqCst));

                // Time the execution took
                let execution_run_time = execution_start_time.elapsed();
                if !block_mode {
                    tracing::info!("Time-based strategy execution ran for {} seconds and intended to run for {} seconds.", execution_run_time.as_secs(), num_blocks_or_seconds);
                }

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

                // For time mode, success_count is just the number of broadcasts
                if !block_mode {
                    success_count = oks.load(Ordering::SeqCst);
                }

                if block_mode {
                    info!(
                        "-----Strategy '{}' completed-----\n\nState transitions attempted: {}\nState \
                        transitions succeeded: {}\nNumber of blocks: {}\nRun time: \
                        {:?} seconds\nDash spent (Loaded Identity): {}\nDash spent (Wallet): {}\n",
                        strategy_name,
                        transition_count,
                        success_count,
                        (current_block_info.height - initial_block_info.height),
                        execution_run_time.as_secs(),
                        dash_spent_identity,
                        dash_spent_wallet,
                    );    
                } else {
                    info!(
                        "-----Strategy '{}' completed-----\n\nState transitions attempted: {}\nState \
                        transitions succeeded: {}\nNumber of loops: {}\nRun time: \
                        {:?} seconds\nAttempted rate (approx): {} txs/s\nDash spent (Loaded Identity): {}\nDash spent (Wallet): {}\n",
                        strategy_name,
                        transition_count,
                        success_count,
                        index-1,
                        execution_run_time.as_secs(),
                        transition_count as u64 / (execution_run_time.as_secs() - 1), // Subtract one here because we sleep for one second after the first block.
                        dash_spent_identity,
                        dash_spent_wallet,
                    );    
                }

                // Make sure we don't divide by 0 when we determine the tx/s rate
                let mut run_time = 1;
                if (execution_run_time.as_secs() - 1) > 1 {
                    run_time = execution_run_time.as_secs() - 1;
                }

                BackendEvent::StrategyCompleted {
                    strategy_name: strategy_name.clone(),
                    result: StrategyCompletionResult::Success {
                        final_block_height: current_block_info.height,
                        start_block_height: initial_block_info.height,
                        success_count: success_count.try_into().unwrap(),
                        transition_count,
                        rate: transition_count as u64 / (run_time - 1),
                        run_time: execution_run_time,
                        dash_spent_identity,
                        dash_spent_wallet,
                    },
                }
            } else {
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
        StrategyTask::ClearContracts(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                // Clear contract_with_updates for the strategy
                strategy.contracts_with_updates.clear();

                // Also clear the displayed contracts
                if let Some(contract_names) = contract_names_lock.get_mut(&strategy_name) {
                    contract_names.clear();
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
