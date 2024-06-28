//! Strategies management backend module.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use dapi_grpc::platform::v0::{
    get_epochs_info_request, get_epochs_info_response,
    wait_for_state_transition_result_response::{
        self, wait_for_state_transition_result_response_v0,
    },
    GetEpochsInfoRequest,
};
use dash_sdk::platform::transition::withdraw_from_identity::WithdrawFromIdentity;
use dash_sdk::{
    platform::{transition::broadcast_request::BroadcastRequestForStateTransition, Fetch},
    Sdk,
};
use dpp::{
    block::{block_info::BlockInfo, epoch::Epoch},
    dashcore::PrivateKey,
    data_contract::{
        accessors::v0::{DataContractV0Getters, DataContractV0Setters},
        created_data_contract::CreatedDataContract,
        document_type::random_document::{DocumentFieldFillSize, DocumentFieldFillType},
        DataContract,
    },
    identity::{
        accessors::IdentityGettersV0, state_transition::asset_lock_proof::AssetLockProof, Identity,
        KeyType, PartialIdentity, Purpose, SecurityLevel,
    },
    platform_value::{string_encoding::Encoding, Identifier},
    serialization::{
        PlatformDeserializableWithPotentialValidationFromVersionedStructure,
        PlatformSerializableWithPlatformVersion,
    },
    state_transition::{
        documents_batch_transition::{
            document_base_transition::v0::v0_methods::DocumentBaseTransitionV0Methods,
            document_transition::DocumentTransition, DocumentCreateTransition,
            DocumentsBatchTransition,
        },
        StateTransition, StateTransitionLike,
    },
    version::PlatformVersion,
};
use drive::{
    drive::{
        document::query::{QueryDocumentsOutcome, QueryDocumentsOutcomeV0Methods},
        identity::key::fetch::IdentityKeysRequest,
        Drive,
    },
    error::proof::ProofError,
    query::DriveQuery,
};
use futures::future::join_all;
use rand::{rngs::StdRng, SeedableRng};
use rs_dapi_client::{DapiRequest, DapiRequestExecutor, RequestSettings};
use simple_signer::signer::SimpleSigner;
use strategy_tests::{
    frequency::Frequency,
    operations::{DocumentAction, DocumentOp, FinalizeBlockOperation, Operation, OperationType},
    IdentityInsertInfo, LocalDocumentQuery, StartIdentities, Strategy, StrategyConfig,
};
use tokio::sync::{Mutex, MutexGuard};

use crate::backend::{wallet::SingleKeyWallet, Wallet};

use super::{
    insight::InsightAPIClient,
    state::{ContractFileName, KnownContractsMap},
    AppState, AppStateUpdate, BackendEvent, StrategyCompletionResult, StrategyContractNames, Task,
};

#[derive(Debug, PartialEq, Clone)]
pub enum StrategyTask {
    CreateStrategy(String),
    ImportStrategy(String),
    ExportStrategy(String),
    SelectStrategy(String),
    DeleteStrategy(String),
    CloneStrategy(String),
    SetStartContracts(String, Vec<String>),
    SetStartContractsRandom(String, String, u8),
    SetIdentityInserts {
        strategy_name: String,
        identity_inserts_frequency: Frequency,
    },
    SetStartIdentities {
        strategy_name: String,
        count: u16,
        keys_count: u8,
        balance: u64,
        add_transfer_key: bool,
    },
    SetStartIdentitiesBalance(String, u64),
    AddOperation {
        strategy_name: String,
        operation: Operation,
    },
    RegisterDocsToAllContracts(String, u16, DocumentFieldFillSize, DocumentFieldFillType),
    RunStrategy(String, u64, bool, bool),
    RemoveLastContract(String),
    ClearContracts(String),
    ClearOperations(String),
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
            match reqwest::get(&url).await {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.bytes().await {
                            Ok(bytes) => {
                                match Strategy::versioned_deserialize(&bytes, true, &sdk.version())
                                {
                                    Ok(strategy) => {
                                        let strategy_name = url.split('/').last()
                                            .map(|s| s.rsplit_once('.').map_or(s, |(name, _)| name))
                                            .map(|s| s.to_string())
                                            .expect("Expected to extract the filename from the imported Strategy file");

                                        let mut strategies_lock =
                                            app_state.available_strategies.lock().await;
                                        strategies_lock
                                            .insert(strategy_name.clone(), strategy.clone());

                                        // We need to add the contracts to available_strategies_contract_names so they can be displayed.
                                        // In order to do so, we need to convert start_contracts into Base58-encoded IDs
                                        let mut strategy_start_contracts_in_format: StrategyContractNames = Vec::new();
                                        for (contract, maybe_updates) in strategy.start_contracts {
                                            let contract_name = contract
                                                .data_contract()
                                                .id()
                                                .to_string(Encoding::Base58);
                                            if let Some(update_map) = maybe_updates {
                                                let formatted_update_map = update_map
                                                    .into_iter()
                                                    .map(|(block_number, created_contract)| {
                                                        let contract_name = created_contract
                                                            .data_contract()
                                                            .id()
                                                            .to_string(Encoding::Base58);
                                                        (block_number, contract_name)
                                                    })
                                                    .collect::<BTreeMap<u64, ContractFileName>>();

                                                strategy_start_contracts_in_format.push((
                                                    contract_name,
                                                    Some(formatted_update_map),
                                                ));
                                            } else {
                                                strategy_start_contracts_in_format
                                                    .push((contract_name, None));
                                            }
                                        }

                                        let mut contract_names_lock = app_state
                                            .available_strategies_contract_names
                                            .lock()
                                            .await;
                                        contract_names_lock.insert(
                                            strategy_name.clone(),
                                            strategy_start_contracts_in_format,
                                        );

                                        let mut selected_strategy =
                                            app_state.selected_strategy.lock().await;
                                        *selected_strategy = Some(strategy_name.clone());

                                        BackendEvent::AppStateUpdated(
                                            AppStateUpdate::SelectedStrategy(
                                                strategy_name.clone(),
                                                MutexGuard::map(strategies_lock, |strategies| {
                                                    strategies.get_mut(&strategy_name).expect("Expected to find the strategy in available_strategies")
                                                }),
                                                MutexGuard::map(contract_names_lock, |names| {
                                                    names.get_mut(&strategy_name).expect("Expected to find the strategy in available_strategies_contract_names")
                                                }),
                                            ),
                                        )
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to deserialize strategy: {}", e);
                                        BackendEvent::StrategyError {
                                            error: format!("Failed to deserialize strategy: {}", e),
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to fetch strategy data: {}", e);
                                BackendEvent::StrategyError {
                                    error: format!("Failed to fetch strategy data: {}", e),
                                }
                            }
                        }
                    } else {
                        tracing::error!("Failed to fetch strategy: HTTP {}", response.status());
                        BackendEvent::StrategyError {
                            error: format!("Failed to fetch strategy: HTTP {}", response.status()),
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to fetch strategy: {}", e);
                    BackendEvent::StrategyError {
                        error: format!("Failed to fetch strategy: {}", e),
                    }
                }
            }
        }
        StrategyTask::ExportStrategy(ref strategy_name) => {
            let strategies_lock = app_state.available_strategies.lock().await;
            let strategy = strategies_lock
                .get(strategy_name)
                .expect("Strategy name doesn't exist in app_state.available_strategies");
            let platform_version = sdk.version();

            match strategy.serialize_to_bytes_with_platform_version(&platform_version) {
                Ok(binary_data) => {
                    let file_name = format!("supporting_files/strategy_exports/{}", strategy_name);
                    let path = std::path::Path::new(&file_name);

                    match File::create(&path) {
                        Ok(mut file) => {
                            if let Err(e) = file.write_all(&binary_data) {
                                tracing::error!("Failed to write strategy to file: {}", e);
                                return BackendEvent::StrategyError {
                                    error: format!("Failed to write strategy to file: {}", e),
                                };
                            }
                            BackendEvent::TaskCompleted {
                                task: Task::Strategy(task),
                                execution_result: Ok(format!(
                                    "Exported strategy file to supporting_files/strategy_exports"
                                )
                                .into()),
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to create file: {}", e);
                            BackendEvent::StrategyError {
                                error: format!("Failed to create file: {}", e),
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to serialize strategy: {}", e);
                    BackendEvent::StrategyError {
                        error: format!("Failed to serialize strategy: {}", e),
                    }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
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
                    BackendEvent::StrategyError {
                        error: format!("Strategy doesn't exist in app state."),
                    }
                }
            } else {
                BackendEvent::StrategyError {
                    error: format!("No selected strategy in app state."),
                }
            }
        }
        StrategyTask::SetStartContracts(strategy_name, selected_contract_names) => {
            // Attain state locks
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let known_contracts_lock = app_state.known_contracts.lock().await;
            let supporting_contracts_lock = app_state.supporting_contracts.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                let platform_version = sdk.version();

                // Function to retrieve the contract from either known_contracts or
                // supporting_contracts
                let get_contract = |contract_name: &String| {
                    known_contracts_lock
                        .get(contract_name)
                        .or_else(|| supporting_contracts_lock.get(contract_name))
                        .cloned()
                };

                // Set a fake identity nonce for now. We will set real identity nonces during strategy execution.
                let fake_identity_nonce = 1;

                if let Some(first_contract_name) = selected_contract_names.first() {
                    if let Some(data_contract) = get_contract(first_contract_name) {
                        match CreatedDataContract::from_contract_and_identity_nonce(
                            data_contract,
                            fake_identity_nonce,
                            platform_version,
                        ) {
                            Ok(original_contract) => {
                                let mut updates = BTreeMap::new();

                                for (order, contract_name) in
                                    selected_contract_names.iter().enumerate().skip(1)
                                {
                                    if let Some(update_contract) = get_contract(contract_name) {
                                        match CreatedDataContract::from_contract_and_identity_nonce(
                                            update_contract,
                                            fake_identity_nonce,
                                            platform_version,
                                        ) {
                                            Ok(created_update_contract) => {
                                                updates
                                                    .insert(order as u64, created_update_contract);
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    "Error converting DataContract to \
                                                     CreatedDataContract for update: {:?}",
                                                    e
                                                );
                                                return BackendEvent::StrategyError {
                                                    error: format!("Error converting DataContract to CreatedDataContract for update: {:?}", e)
                                                };
                                            }
                                        }
                                    }
                                }

                                strategy.start_contracts.push((
                                    original_contract,
                                    if updates.is_empty() {
                                        None
                                    } else {
                                        Some(updates)
                                    },
                                ));
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Error converting DataContract to CreatedDataContract: {:?}",
                                    e
                                );
                                return BackendEvent::StrategyError {
                                    error: format!("Error converting DataContract to CreatedDataContract: {:?}", e)
                                };
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
            }
        }
        StrategyTask::SetStartContractsRandom(strategy_name, selected_contract_name, variants) => {
            // Attain state locks
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let known_contracts_lock = app_state.known_contracts.lock().await;
            let mut supporting_contracts_lock = app_state.supporting_contracts.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                let platform_version = sdk.version();

                // Function to retrieve the contract from either known_contracts or
                // supporting_contracts
                let get_contract = |contract_name: &String| {
                    known_contracts_lock
                        .get(contract_name)
                        .or_else(|| supporting_contracts_lock.get(contract_name))
                        .cloned()
                };

                // Set a fake identity nonce for now. We will set real identity nonces during strategy execution.
                let mut fake_identity_nonce = 1;

                // Add the contracts to the strategy start_contracts
                if let Some(data_contract) = get_contract(&selected_contract_name) {
                    match CreatedDataContract::from_contract_and_identity_nonce(
                        data_contract,
                        fake_identity_nonce,
                        platform_version,
                    ) {
                        Ok(original_contract) => {
                            // Add original contract to the strategy
                            let mut contract_variants: Vec<CreatedDataContract> = Vec::new();
                            contract_variants.push(original_contract.clone());
                            strategy
                                .start_contracts
                                .push((original_contract.clone(), None));

                            // Add i variants of the original contract to the strategy
                            for i in 0..variants - 1 {
                                let mut new_data_contract =
                                    original_contract.data_contract().clone();
                                let new_id = DataContract::generate_data_contract_id_v0(
                                    Identifier::random(),
                                    fake_identity_nonce,
                                );
                                new_data_contract.set_id(new_id);
                                match CreatedDataContract::from_contract_and_identity_nonce(
                                    new_data_contract.clone(),
                                    fake_identity_nonce,
                                    platform_version,
                                ) {
                                    Ok(contract) => {
                                        contract_variants.push(contract.clone());
                                        strategy.start_contracts.push((contract, None));
                                        let new_contract_name = String::from(format!(
                                            "{}_variant_{}",
                                            selected_contract_name, i
                                        ));
                                        // Insert into supporting_contracts so we can register documents to them. We will clear
                                        // supporting contracts at the end of strategy execution.
                                        supporting_contracts_lock
                                            .insert(new_contract_name, new_data_contract);
                                        fake_identity_nonce += 1;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Error converting DataContract to CreatedDataContract variant: {:?}",
                                            e
                                        );
                                        return BackendEvent::StrategyError {
                                            error: format!("Error converting DataContract to CreatedDataContract variant: {:?}", e)
                                        };
                                    }
                                };
                            }

                            let contract_id_strings: Vec<(String, Option<BTreeMap<u64, String>>)> =
                                contract_variants
                                    .iter()
                                    .map(|x| {
                                        (x.data_contract().id().to_string(Encoding::Base58), None)
                                    })
                                    .collect();

                            // Add the new contracts to app_state.available_strategies_contract_names
                            if let Some(existing_strategy_contracts) =
                                contract_names_lock.get_mut(&strategy_name)
                            {
                                existing_strategy_contracts.extend(contract_id_strings);
                            } else {
                                contract_names_lock
                                    .insert(strategy_name.clone(), contract_id_strings);
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "Error converting original DataContract to CreatedDataContract: {:?}",
                                e
                            );
                            return BackendEvent::StrategyError {
                                error: format!("Error converting original DataContract to CreatedDataContract: {:?}", e)
                            };
                        }
                    }
                } else {
                    tracing::error!("Contract wasn't retrieved by name in StrategyTask::SetContractsWithUpdatesRandom");
                    return BackendEvent::StrategyError {
                        error: format!("Contract wasn't retrieved by name in StrategyTask::SetContractsWithUpdatesRandom")
                    };
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
            }
        }
        StrategyTask::RegisterDocsToAllContracts(strategy_name, num_docs, fill_size, fill_type) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                for contract_with_updates in &strategy.start_contracts {
                    let contract = &contract_with_updates.0;
                    let document_types = contract.data_contract().document_types();
                    let document_type = document_types
                        .values()
                        .next()
                        .expect("Expected to get a document type in RegisterDocsToAllContracts");
                    let action = DocumentAction::DocumentActionInsertRandom(fill_type, fill_size);
                    let operation = Operation {
                        op_type: OperationType::Document(DocumentOp {
                            contract: contract.data_contract().clone(),
                            document_type: document_type.clone(),
                            action,
                        }),
                        frequency: Frequency {
                            times_per_block_range: num_docs..num_docs + 1,
                            chance_per_block: None,
                        },
                    };
                    strategy.operations.push(operation);
                }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
            }
        }
        StrategyTask::SetIdentityInserts {
            strategy_name,
            identity_inserts_frequency,
        } => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.identity_inserts = IdentityInsertInfo {
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
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
                    extra_keys.insert(
                        Purpose::TRANSFER,
                        [(SecurityLevel::CRITICAL, vec![KeyType::ECDSA_SECP256K1])].into(),
                    );
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state."),
                }
            }
        }
        StrategyTask::RunStrategy(
            strategy_name,
            num_blocks_or_seconds,
            verify_proofs,
            block_mode,
        ) => {
            tracing::info!("-----Starting strategy '{}'-----", strategy_name);
            let init_start_time = Instant::now(); // Start time of strategy initialization plus execution of first two blocks
            let mut init_time = Duration::new(0, 0); // Will set this to the time it takes for all initialization plus the first two blocks to complete

            // Fetch known_contracts from the chain to assure local copies match actual state.
            match update_known_contracts(sdk, &app_state.known_contracts).await {
                Ok(_) => {
                    // nothing
                }
                Err(e) => {
                    tracing::error!("Failed to update known contracts: {:?}", e);
                    return BackendEvent::StrategyError {
                        error: format!("Failed to update known contracts: {:?}", e),
                    };
                }
            };

            // Refresh loaded_identity and get the current balance at strategy start
            let mut loaded_identity_lock = match app_state.refresh_identity(&sdk).await {
                Ok(lock) => lock,
                Err(e) => {
                    tracing::error!("Failed to refresh loaded identity: {:?}", e);
                    return BackendEvent::StrategyError {
                        error: format!("Failed to refresh loaded identity: {:?}", e),
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
                            tracing::error!("Error executing request, retrying: {:?}", e);
                            retries += 1;
                        }
                        Err(e) => {
                            tracing::error!("Failed to execute request after retries: {:?}", e);
                            return BackendEvent::StrategyError {
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
                let used_contract_ids = strategy.used_contract_ids();
                let mut identity_nonce_counter = BTreeMap::new();
                let mut contract_nonce_counter: BTreeMap<(Identifier, Identifier), u64> =
                    BTreeMap::new();
                let num_contract_create_operations = strategy
                    .operations
                    .iter()
                    .filter(|op| matches!(op.op_type, OperationType::ContractCreate(_, _)))
                    .count();
                if used_contract_ids.len() + num_contract_create_operations > 0 {
                    tracing::info!(
                        "Fetching identity nonce and {} identity contract nonces from Platform...",
                        used_contract_ids.len()
                    );
                    let nonce_fetching_time = Instant::now();
                    let identity_future = sdk.get_identity_nonce(
                        loaded_identity_clone.id(),
                        false,
                        Some(dash_sdk::platform::transition::put_settings::PutSettings {
                            request_settings: RequestSettings::default(),
                            identity_nonce_stale_time_s: Some(0),
                            user_fee_increase: None,
                        }),
                    );
                    let contract_futures =
                        used_contract_ids
                            .clone()
                            .into_iter()
                            .map(|used_contract_id| {
                                let identity_id = loaded_identity_clone.id();
                                async move {
                                    let current_nonce = sdk.get_identity_contract_nonce(
                                identity_id,
                                used_contract_id,
                                false,
                                Some(dash_sdk::platform::transition::put_settings::PutSettings {
                                    request_settings: RequestSettings::default(),
                                    identity_nonce_stale_time_s: Some(0),
                                    user_fee_increase: None,
                                })
                            ).await.expect("Couldn't get current identity contract nonce");
                                    ((identity_id, used_contract_id), current_nonce)
                                }
                            });
                    let identity_result = identity_future
                        .await
                        .expect("Couldn't get current identity nonce");
                    identity_nonce_counter.insert(loaded_identity_clone.id(), identity_result);
                    let contract_results = join_all(contract_futures).await;
                    contract_nonce_counter = contract_results.into_iter().collect();
                    tracing::info!(
                        "Took {} seconds to obtain {} identity contract nonces",
                        nonce_fetching_time.elapsed().as_secs(),
                        used_contract_ids.len()
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
                            let drive_query =
                                DriveQuery::any_item_query(data_contract, document_type.as_ref());

                            // Query the Drive for documents
                            match drive_lock.query_documents(drive_query, None, false, None, None) {
                                Ok(outcome) => match outcome {
                                    QueryDocumentsOutcome::V0(outcome_v0) => {
                                        let documents = outcome_v0.documents_owned();
                                        tracing::info!(
                                            "Fetched {} documents using DriveQuery",
                                            documents.len()
                                        );
                                        documents
                                    }
                                },
                                Err(e) => {
                                    tracing::error!(
                                        "Error fetching documents using DriveQuery: {:?}",
                                        e
                                    );
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
                            sdk.version(),
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
                                tracing::info!(
                                    "Fetched identity info for identifier {}: {:?}",
                                    identifier,
                                    partial_identity
                                );
                                partial_identity
                            }
                            Err(e) => {
                                tracing::error!("Error fetching identity: {:?}", e);
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

                // Create asset lock proofs for all the identity creates and top ups
                let num_identity_inserts = (strategy
                    .identity_inserts
                    .frequency
                    .times_per_block_range
                    .start) as u64;
                let num_start_identities = strategy.start_identities.number_of_identities as u64;
                let mut num_top_ups: u64 = 0;
                for operation in &strategy.operations {
                    if operation.op_type == OperationType::IdentityTopUp {
                        num_top_ups += (operation.frequency.times_per_block_range.start) as u64;
                    }
                }
                let num_asset_lock_proofs_needed = num_identity_inserts * num_blocks_or_seconds
                    + num_start_identities
                    + num_top_ups;
                let mut asset_lock_proofs: Vec<(AssetLockProof, PrivateKey)> = Vec::new();
                if num_asset_lock_proofs_needed > 0 {
                    let mut wallet_lock = app_state.loaded_wallet.lock().await;
                    let num_available_utxos = match wallet_lock
                        .clone()
                        .expect("No wallet loaded while getting asset lock proofs")
                    {
                        Wallet::SingleKeyWallet(SingleKeyWallet { utxos, .. }) => utxos.len(),
                    };
                    if num_available_utxos
                        < num_asset_lock_proofs_needed
                            .try_into()
                            .expect("Couldn't convert num_asset_lock_proofs_needed into usize")
                    {
                        return BackendEvent::StrategyError {
                            error: format!("Not enough UTXOs available in wallet. Available: {}. Need: {}. Go to Wallet screen and create more.", num_available_utxos, num_asset_lock_proofs_needed),
                        };
                    }
                    tracing::info!(
                        "Obtaining {} asset lock proofs for the strategy...",
                        num_asset_lock_proofs_needed
                    );
                    let asset_lock_proof_time = Instant::now();
                    let mut num_asset_lock_proofs_obtained = 0;
                    for i in 0..(num_asset_lock_proofs_needed) {
                        if let Some(wallet) = wallet_lock.as_mut() {
                            // TO-DO: separate this into a function for start_identities, top ups, and inserts, because the balances should all be different
                            match wallet.asset_lock_transaction(
                                None,
                                strategy.start_identities.starting_balances,
                            ) {
                                Ok((asset_lock_transaction, asset_lock_proof_private_key)) => {
                                    match AppState::broadcast_and_retrieve_asset_lock(
                                        sdk,
                                        &asset_lock_transaction,
                                        &wallet.receive_address(),
                                    )
                                    .await
                                    {
                                        Ok(asset_lock_proof) => {
                                            tracing::info!(
                                                "Successfully obtained asset lock proof number {}",
                                                i + 1
                                            );
                                            num_asset_lock_proofs_obtained += 1;
                                            asset_lock_proofs.push((
                                                asset_lock_proof,
                                                asset_lock_proof_private_key,
                                            ));
                                        }
                                        Err(_) => {
                                            // this error is handled from within the function
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Error creating asset lock transaction: {:?}",
                                        e
                                    )
                                }
                            }
                        } else {
                            tracing::error!("Wallet not loaded");
                        }
                    }
                    tracing::info!(
                        "Took {} seconds to obtain {} asset lock proofs",
                        asset_lock_proof_time.elapsed().as_secs(),
                        num_asset_lock_proofs_obtained
                    );
                    drop(wallet_lock); // I don't think this is necessary anymore after moving into the if statement
                }

                // Get the execution mode as a string for logging
                let mut mode_string = String::new();
                if block_mode {
                    mode_string.push_str("block");
                } else {
                    mode_string.push_str("second");
                }

                // Some final initialization
                let mut rng = StdRng::from_entropy(); // Will be passed to state_transitions_for_block
                let mut current_block_info = initial_block_info.clone(); // Used for transition creation and logging
                let mut transition_count = 0; // Used for logging how many transitions we attempted
                let mut success_count = 0; // Used for logging how many transitions were successful
                let mut load_start_time = Instant::now(); // Time when the load test begins (all blocks after the second block)
                let mut index = 1; // Index of the loop iteration. Represents blocks for block mode and seconds for time mode
                let mut new_identity_ids = Vec::new(); // Will capture the ids of identities added to current_identities
                let mut new_contract_ids = Vec::new(); // Will capture the ids of newly created data contracts
                let oks = Arc::new(AtomicUsize::new(0)); // Atomic counter for successful broadcasts
                let errs = Arc::new(AtomicUsize::new(0)); // Atomic counter for failed broadcasts
                let mempool_document_counter = BTreeMap::new();

                // Now loop through the number of blocks or seconds the user asked for, preparing and processing state transitions
                while (block_mode && current_block_info.height < (initial_block_info.height + num_blocks_or_seconds + 2)) // +2 because we don't count the first two initialization blocks
                    || (!block_mode && load_start_time.elapsed().as_secs() < num_blocks_or_seconds) || index <= 2
                {
                    let oks_clone = oks.clone();
                    let errs_clone = errs.clone();
                    let loop_start_time = Instant::now();

                    // Need to pass app_state.known_contracts to state_transitions_for_block
                    let mut known_contracts_lock = app_state.known_contracts.lock().await;

                    // Get the state transitions for the block (or second)
                    let (transitions, finalize_operations, mut new_identities) = strategy
                        .state_transitions_for_block(
                            &mut document_query_callback,
                            &mut identity_fetch_callback,
                            &mut asset_lock_proofs,
                            &current_block_info,
                            &mut current_identities,
                            &mut known_contracts_lock,
                            &mut signer,
                            &mut identity_nonce_counter,
                            &mut contract_nonce_counter,
                            mempool_document_counter.clone(),
                            &mut rng,
                            &StrategyConfig {
                                start_block_height: initial_block_info.height,
                                number_of_blocks: num_blocks_or_seconds,
                            },
                            sdk.version(),
                        )
                        .await;

                    drop(known_contracts_lock);

                    // Add the identities that will be created to current_identities.
                    // TO-DO: This should be moved to execution after we confirm they were registered.
                    for identity in &new_identities {
                        new_identity_ids.push(identity.id().to_string(Encoding::Base58))
                    }
                    current_identities.append(&mut new_identities);

                    for transition in &transitions {
                        match transition {
                            StateTransition::DataContractCreate(contract_create_transition) => {
                                new_contract_ids.extend(
                                    contract_create_transition
                                        .modified_data_ids()
                                        .iter()
                                        .map(|id| id.to_string(Encoding::Base58)),
                                );
                            }
                            _ => {
                                // nothing
                            }
                        };
                    }

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
                                                                    tracing::info!("Successfully processed state transition {} ({}) for {} {} (Actual block height: {})", st_queue_index, transition_type, mode_string, index, metadata.height);
                                                                }
                                                                match &v0_response.result {
                                                                    Some(wait_for_state_transition_result_response_v0::Result::Error(error)) => {
                                                                        tracing::error!("WaitForStateTransitionResultResponse error: {:?}", error);
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
                                                                                    tracing::info!("Successfully processed and verified proof for state transition {} ({}), {} {} (Actual block height: {})", st_queue_index, transition_type, mode_string, index, metadata.height);
                                                                                }
                                                                                Err(e) => {
                                                                                    tracing::error!("Error verifying state transition execution proof: {}", e);
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
                                                            tracing::info!("Response version other than V0 received or absent for state transition {} ({})", st_queue_index, transition_type);
                                                        }
                                                    }
                                                    Err(e) => tracing::error!(
                                                        "Error waiting for state transition result: {:?}",
                                                        e
                                                    ),
                                                }
                                            } else {
                                                tracing::error!(
                                                    "Failed to create wait request for state transition."
                                                );
                                            }
                                        }
                                        Err(e) => tracing::error!(
                                            "Error broadcasting dependent state transition: {:?}",
                                            e
                                        ),
                                    }
                                } else {
                                    tracing::error!(
                                        "Failed to create broadcast request for state transition."
                                    );
                                }
                            } else {
                                let oks = oks_clone.clone();
                                let errs = errs_clone.clone();

                                let mut request_settings = RequestSettings::default();
                                if !block_mode && index != 1 && index != 2 {
                                    request_settings.connect_timeout = Some(Duration::from_secs(1));
                                    request_settings.timeout = Some(Duration::from_secs(1));
                                    request_settings.retries = Some(0);
                                }
                                if block_mode && index != 1 && index != 2 {
                                    request_settings.connect_timeout = Some(Duration::from_secs(3));
                                    request_settings.timeout = Some(Duration::from_secs(3));
                                    request_settings.retries = Some(1);
                                }

                                // Prepare futures for broadcasting independent transitions
                                let future = async move {
                                    match transition_clone.broadcast_request_for_state_transition() {
                                        Ok(broadcast_request) => {
                                            let broadcast_result = broadcast_request.execute(sdk, request_settings).await;
                                            match broadcast_result {
                                                Ok(_) => {
                                                    oks.fetch_add(1, Ordering::SeqCst);
                                                    if !block_mode && index != 1 && index != 2 {
                                                        tracing::info!("Successfully broadcasted transition: {}", transition_clone.name());
                                                    }
                                                    Ok((transition_clone, broadcast_result))
                                                },
                                                Err(e) => {
                                                    errs.fetch_add(1, Ordering::SeqCst);
                                                    // Error logging seems unnecessary here because rs-dapi-client seems to log all the errors already
                                                    // But it is necessary. `rs-dapi-client` does not log all the errors already. For example, IdentityNotFound errors
                                                    tracing::error!("Failed to broadcast transition: {}, Error: {:?}", transition_clone.name(), e);
                                                    Err(e)
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            errs.fetch_add(1, Ordering::SeqCst);
                                            if !block_mode {
                                                tracing::error!("Error preparing broadcast request for transition: {}, Error: {:?}", transition_clone.name(), e);
                                            }
                                            Err(e)
                                        }.expect("Expected to prepare broadcast for request for state transition") // I guess I have to do this to make it compile
                                    }
                                };
                                broadcast_futures.push(future);
                            }
                        }

                        // Concurrently execute all broadcast requests for independent transitions
                        let broadcast_results = join_all(broadcast_futures).await;

                        // If we're in block mode, or index 1 or 2 of time mode, we're going to wait for state transition results and potentially verify proofs too.
                        // If we're in time mode and index 3+, we're just broadcasting.
                        if block_mode || index == 1 || index == 2 {
                            let mut wait_futures = Vec::new();
                            for (index, result) in broadcast_results.into_iter().enumerate() {
                                match result {
                                    Ok((transition, broadcast_result)) => {
                                        let transition_type = transition.name().to_owned();

                                        if broadcast_result.is_err() {
                                            tracing::error!(
                                                "Error broadcasting state transition {} ({}) for {} {}: {:?}",
                                                index + 1,
                                                transition_type,
                                                mode_string,
                                                index,
                                                broadcast_result.err().unwrap()
                                            );
                                            continue;
                                        }

                                        // I think what's happening here is we're using this data_contract_clone for proof verification later
                                        let data_contract_id_option = match &transition {
                                            StateTransition::DocumentsBatch(
                                                DocumentsBatchTransition::V0(documents_batch),
                                            ) => {
                                                documents_batch.transitions.get(0).and_then(
                                                    |document_transition| {
                                                        match document_transition {
                                                            DocumentTransition::Create(
                                                                DocumentCreateTransition::V0(
                                                                    create_transition,
                                                                ),
                                                            ) => Some(
                                                                create_transition
                                                                    .base
                                                                    .data_contract_id(),
                                                            ),
                                                            // Add handling for Replace and Delete transitions if necessary
                                                            _ => None,
                                                        }
                                                    },
                                                )
                                            }
                                            // Handle other state transition types that involve data contracts here
                                            _ => None,
                                        };
                                        let known_contracts_lock =
                                            app_state.known_contracts.lock().await;
                                        let data_contract_clone = if let Some(data_contract_id) =
                                            data_contract_id_option
                                        {
                                            let data_contract_id_str =
                                                data_contract_id.to_string(Encoding::Base58);
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
                                            let wait_result = match transition
                                                .wait_for_state_transition_result_request()
                                            {
                                                Ok(wait_request) => {
                                                    wait_request
                                                        .execute(sdk, RequestSettings::default())
                                                        .await
                                                }
                                                Err(e) => {
                                                    tracing::error!(
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
                                                                tracing::info!(
                                                                    "Successfully broadcasted and processed state transition {} ({}) for {} {} (Actual block height: {})",
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
                                                                            tracing::info!("Successfully processed and verified proof for state transition {} ({}), {} {} (Actual block height: {})", index + 1, transition_type, mode_string, index, metadata.height);
                                                                        }
                                                                        Err(e) => tracing::error!("Error verifying state transition execution proof: {}", e),
                                                                    }
                                                                }
                                                            }

                                                            // Log the Base58 encoded IDs of any created contracts or identities
                                                            match transition.clone() {
                                                                StateTransition::IdentityCreate(identity_create_transition) => {
                                                                    let ids = identity_create_transition.modified_data_ids();
                                                                    for id in ids {
                                                                        let encoded_id: String = id.into();
                                                                        tracing::info!("Created identity: {}", encoded_id);
                                                                    }
                                                                },
                                                                StateTransition::DataContractCreate(contract_create_transition) => {
                                                                    let ids = contract_create_transition.modified_data_ids();
                                                                    for id in ids {
                                                                        let encoded_id: String = id.into();
                                                                        tracing::info!("Created contract: {}", encoded_id);
                                                                    }
                                                                },
                                                                _ => {
                                                                    // nothing
                                                                }
                                                            }
                                                        }
                                                    })
                                                }
                                                Err(e) => {
                                                    tracing::error!("Wait result error: {:?}", e);
                                                    None
                                                }
                                            }
                                        };
                                        wait_futures.push(wait_future);
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Error preparing broadcast request for state transition {} {} {}: {:?}",
                                            index + 1,
                                            mode_string,
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
                        } else {
                            // Time mode.
                            // Don't wait.
                        }

                        // Reset the load_start_time
                        if index == 1 || index == 2 {
                            load_start_time = Instant::now();
                        }
                    } else {
                        // No transitions prepared for the block (or second)
                        tracing::info!(
                            "Prepared 0 state transitions for {} {}",
                            mode_string,
                            index
                        );
                    }

                    if index == 2 {
                        init_time = init_start_time.elapsed();
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
                tracing::info!(
                    "Successfully processed: {}, Failed to process: {}",
                    oks.load(Ordering::SeqCst),
                    errs.load(Ordering::SeqCst)
                );

                // Time the execution took
                let load_execution_run_time = load_start_time.elapsed();
                if !block_mode {
                    tracing::info!("Time-based strategy execution ran for {} seconds and intended to run for {} seconds.", load_execution_run_time.as_secs(), num_blocks_or_seconds);
                }

                // Log all the newly created identities and contracts.
                // Note these txs were not confirmed. They were just attempted at least.
                tracing::info!(
                    "Newly created identities (attempted): {:?}",
                    new_identity_ids
                );
                tracing::info!(
                    "Newly created contracts (attempted): {:?}",
                    new_contract_ids
                );

                // Withdraw all funds from newly created identities back to the wallet
                current_identities.remove(0); // Remove loaded identity from the vector
                let wallet_lock = app_state
                    .loaded_wallet
                    .lock()
                    .await
                    .clone()
                    .expect("Expected a loaded wallet while withdrawing");
                tracing::info!("Withdrawing funds from newly created identities back to the loaded wallet (if they have transfer keys)...");
                let mut withdrawals_count = 0;
                for identity in current_identities {
                    if identity
                        .get_first_public_key_matching(
                            Purpose::TRANSFER,
                            [SecurityLevel::CRITICAL].into(),
                            KeyType::all_key_types().into(),
                        )
                        .is_some()
                    {
                        let result = identity
                            .withdraw(
                                sdk,
                                wallet_lock.receive_address(),
                                identity.balance() - 1_000_000, // not sure what this should be
                                None,
                                None,
                                signer.clone(),
                                None,
                            )
                            .await;
                        match result {
                            Ok(balance) => {
                                tracing::info!(
                                    "Withdrew {} from identity {}",
                                    balance,
                                    identity.id().to_string(Encoding::Base58)
                                );
                                withdrawals_count += 1;
                            }
                            Err(e) => {
                                if e.to_string().contains("invalid proof") {
                                    tracing::info!(
                                        "Withdrew from identity {} but proof not verified",
                                        identity.id().to_string(Encoding::Base58)
                                    );
                                    withdrawals_count += 1;
                                } else {
                                    tracing::error!(
                                        "Error withdrawing from identity {}: {}",
                                        identity.id().to_string(Encoding::Base58),
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
                tracing::info!("Completed {} withdrawals.", withdrawals_count);
                drop(wallet_lock);

                // Refresh the identity at the end
                drop(loaded_identity_lock);
                let refresh_result = app_state.refresh_identity(&sdk).await;
                if let Err(ref e) = refresh_result {
                    tracing::error!("Failed to refresh identity after running strategy: {:?}", e);
                }

                // Attempt to retrieve the final balance from the refreshed identity
                let final_balance_identity = match refresh_result {
                    Ok(refreshed_identity_lock) => {
                        // Successfully refreshed, now access the balance
                        refreshed_identity_lock.balance()
                    }
                    Err(_) => {
                        tracing::error!("Error refreshing identity after running strategy");
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

                // Make sure we don't divide by 0 when we determine the tx/s rate
                let mut load_run_time = 1;
                if (load_execution_run_time.as_secs()) > 1 {
                    load_run_time = load_execution_run_time.as_secs();
                }

                // Calculate transactions per second
                let mut tps = 0;
                if transition_count
                    > (strategy.start_contracts.len() as u16
                        + strategy.start_identities.number_of_identities as u16)
                {
                    tps = (transition_count
                        - strategy.start_contracts.len() as u16
                        - strategy.start_identities.number_of_identities as u16)
                        as u64
                        / (load_run_time)
                };
                let mut successful_tps = 0;
                if success_count as u16
                    > (strategy.start_contracts.len() as u16
                        + strategy.start_identities.number_of_identities as u16)
                {
                    successful_tps = (success_count
                        - strategy.start_contracts.len()
                        - strategy.start_identities.number_of_identities as usize)
                        as u64
                        / (load_run_time)
                };
                let mut success_percent = 0;
                if success_count as u16
                    > (strategy.start_contracts.len() as u16
                        + strategy.start_identities.number_of_identities as u16)
                {
                    success_percent = (((success_count
                        - strategy.start_contracts.len()
                        - strategy.start_identities.number_of_identities as usize)
                        as f64
                        / (transition_count
                            - strategy.start_contracts.len() as u16
                            - strategy.start_identities.number_of_identities as u16)
                            as f64)
                        * 100.0) as u64
                };

                // Clear app_state.supporting_contracts
                let mut supporting_contracts_lock = app_state.supporting_contracts.lock().await;
                supporting_contracts_lock.clear();

                if block_mode {
                    tracing::info!(
                        "-----Strategy '{}' completed-----\n\nMode: {}\nState transitions attempted: {}\nState \
                        transitions succeeded: {}\nNumber of blocks: {}\nRun time: \
                        {:?} seconds\nTPS rate (approx): {} tps\nDash spent (Loaded Identity): {}\nDash spent (Wallet): {}\n",
                        strategy_name,
                        mode_string,
                        transition_count,
                        success_count,
                        (current_block_info.height - initial_block_info.height),
                        load_run_time, // Processing time after the second block
                        tps, // tps besides the first two blocks
                        dash_spent_identity,
                        dash_spent_wallet,
                    );
                } else {
                    // Time mode
                    tracing::info!(
                        "-----Strategy '{}' completed-----\n\nMode: {}\nState transitions attempted: {}\nState \
                        transitions succeeded: {}\nNumber of loops: {}\nLoad run time: \
                        {:?} seconds\nInit run time: {} seconds\nAttempted rate (approx): {} txs/s\nSuccessful rate: {} tx/s\nSuccess percentage: {}%\nDash spent (Loaded Identity): {}\nDash spent (Wallet): {}\n",
                        strategy_name,
                        mode_string,
                        transition_count,
                        success_count,
                        index-3, // Minus 3 because we still incremented one at the end of the last loop, and don't count the first two blocks
                        load_run_time,
                        init_time.as_secs(),
                        tps,
                        successful_tps,
                        success_percent,
                        dash_spent_identity,
                        dash_spent_wallet,
                    );
                }

                BackendEvent::StrategyCompleted {
                    strategy_name: strategy_name.clone(),
                    result: StrategyCompletionResult::Success {
                        block_mode: block_mode,
                        final_block_height: current_block_info.height,
                        start_block_height: initial_block_info.height,
                        success_count: success_count.try_into().unwrap(),
                        transition_count: transition_count.try_into().unwrap(),
                        rate: tps,
                        success_rate: successful_tps,
                        success_percent: success_percent,
                        run_time: load_execution_run_time,
                        init_time: init_time,
                        dash_spent_identity,
                        dash_spent_wallet,
                    },
                }
            } else {
                tracing::error!("No strategy loaded with name \"{}\"", strategy_name);
                BackendEvent::StrategyError {
                    error: format!("No strategy loaded with name \"{}\"", strategy_name),
                }
            }
        }
        StrategyTask::RemoveLastContract(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                // Remove the last contract_with_update entry from the strategy
                strategy.start_contracts.pop();

                // Also remove the corresponding entry from the displayed contracts
                if let Some(contract_names) = contract_names_lock.get_mut(&strategy_name) {
                    // Assuming each entry in contract_names corresponds to an entry in
                    // start_contracts
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state"),
                }
            }
        }
        StrategyTask::ClearContracts(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let mut contract_names_lock =
                app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                // Clear contract_with_updates for the strategy
                strategy.start_contracts.clear();

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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state"),
                }
            }
        }
        StrategyTask::ClearOperations(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            let contract_names_lock = app_state.available_strategies_contract_names.lock().await;

            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                // Clear operations for the strategy
                strategy.operations.clear();

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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state"),
                }
            }
        }
        StrategyTask::RemoveIdentityInserts(strategy_name) => {
            let mut strategies_lock = app_state.available_strategies.lock().await;
            if let Some(strategy) = strategies_lock.get_mut(&strategy_name) {
                strategy.identity_inserts = IdentityInsertInfo::default();
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state"),
                }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state"),
                }
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
                BackendEvent::StrategyError {
                    error: format!("Strategy doesn't exist in app state"),
                }
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
                tracing::error!("Contract not found for ID: {}", contract_id_str);
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
