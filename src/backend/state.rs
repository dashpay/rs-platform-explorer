//! Application state module.
//! This kind of state does not include UI details and basically all about
//! persistence required by backend.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::BTreeMap, fs};

use bincode::{Decode, Encode};
use dpp::{
    dashcore::{
        psbt::serialize::{Deserialize, Serialize},
        Network, PrivateKey, Transaction,
    },
    identity::{IdentityPublicKey, KeyID},
    prelude::{AssetLockProof, DataContract, Identifier, Identity},
    serialization::{
        PlatformDeserializableWithPotentialValidationFromVersionedStructure,
        PlatformSerializableWithPlatformVersion,
    },
    tests::json_document::json_document_to_contract,
    util::deserializer::ProtocolVersion,
    version::PlatformVersion,
    ProtocolError::{self, PlatformDeserializationError, PlatformSerializationError},
};
use drive::drive::Drive;
use strategy_tests::Strategy;
use tokio::sync::Mutex;
use walkdir::{DirEntry, WalkDir};

use super::wallet::{add_wallet_by_private_key, Wallet};
use crate::{backend::insight::InsightAPIClient, config::Config};

const CURRENT_PROTOCOL_VERSION: ProtocolVersion = 1;

const _USE_LOCAL: bool = false;

pub(crate) type ContractFileName = String;

pub(super) type StrategiesMap = BTreeMap<String, Strategy>;
pub(crate) type StrategyContractNames =
    Vec<(ContractFileName, Option<BTreeMap<u64, ContractFileName>>)>;
pub(super) type KnownContractsMap = BTreeMap<String, DataContract>;
pub type IdentityPrivateKeysMap = BTreeMap<(Identifier, KeyID), Vec<u8>>;

// TODO: each state part should be in it's own mutex in case multiple backend
// tasks are executed on different state parts,
// moreover single mutex hold during rendering will block unrelated tasks from
// finishing
// #[derive(Debug)]
pub struct AppState {
    pub loaded_identity: Mutex<Option<Identity>>,
    pub identity_private_keys: Mutex<IdentityPrivateKeysMap>,
    pub loaded_wallet: Mutex<Option<Wallet>>,
    pub drive: Mutex<Drive>,
    pub known_identities: Mutex<BTreeMap<Identifier, Identity>>,
    pub known_contracts: Mutex<KnownContractsMap>,
    pub supporting_contracts: Mutex<BTreeMap<String, DataContract>>, /* Contracts from
                                                                      * supporting_files */
    pub available_strategies: Mutex<StrategiesMap>,
    /// Because we don't store which contract support file was used exactly we
    /// cannot properly restore the state and display a strategy, so this
    /// field serves as a double of strategies' `start_contracts`,
    /// but using file names
    pub available_strategies_contract_names: Mutex<BTreeMap<String, StrategyContractNames>>,
    pub selected_strategy: Mutex<Option<String>>,
    pub identity_asset_lock_private_key_in_creation: Mutex<
        Option<(
            Transaction,
            PrivateKey,
            Option<AssetLockProof>,
            Option<(Identity, BTreeMap<IdentityPublicKey, Vec<u8>>)>,
        )>,
    >,
    pub identity_asset_lock_private_key_in_top_up:
        Mutex<Option<(Transaction, PrivateKey, Option<AssetLockProof>)>>,
}

impl Default for AppState {
    fn default() -> Self {
        let platform_version = PlatformVersion::get(CURRENT_PROTOCOL_VERSION).unwrap();

        // Helper function to check if a file is a JSON file
        fn is_json(entry: &DirEntry) -> bool {
            entry.path().extension().and_then(|s| s.to_str()) == Some("json")
        }

        // Initialize supporting_contracts
        let mut supporting_contracts_raw = BTreeMap::new();

        // Iterate over JSON files in the supporting_files/contract directory
        for entry in WalkDir::new("supporting_files/contract")
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(is_json)
        {
            let path = entry.path();
            let contract_name = path.file_stem().unwrap().to_str().unwrap().to_string();

            if let Ok(contract) = json_document_to_contract(&path, true, platform_version) {
                // Insert the contract into supporting_contracts_raw
                supporting_contracts_raw.insert(contract_name, contract);
            }
        }

        let (drive, _protocol_version) =
            Drive::open("explorer.drive", None).expect("expected to open Drive successfully");

        if drive
            .grove
            .is_empty_tree(drive::grovedb_path::SubtreePath::empty(), None)
            .unwrap()
            .expect("expected to find id this is an empty db")
        {
            drive
                .create_initial_state_structure(None, platform_version)
                .expect("expected to create root tree successfully");
        }

        AppState {
            loaded_identity: None.into(),
            identity_private_keys: Default::default(),
            loaded_wallet: Mutex::new(None),
            drive: Mutex::from(drive),
            known_contracts: BTreeMap::new().into(),
            supporting_contracts: supporting_contracts_raw.into(),
            known_identities: BTreeMap::new().into(),
            available_strategies: BTreeMap::new().into(),
            selected_strategy: None.into(),
            identity_asset_lock_private_key_in_creation: None.into(),
            identity_asset_lock_private_key_in_top_up: None.into(),
            available_strategies_contract_names: BTreeMap::new().into(),
        }
    }
}

#[derive(Clone, Debug, Encode, Decode)]
struct AppStateInSerializationFormat {
    pub loaded_identity: Option<Identity>,
    pub identity_private_keys: IdentityPrivateKeysMap,
    pub loaded_wallet: Option<Wallet>,
    pub known_identities: BTreeMap<Identifier, Identity>,
    pub known_contracts: BTreeMap<String, Vec<u8>>,
    pub supporting_contracts: BTreeMap<String, Vec<u8>>,
    pub available_strategies: BTreeMap<String, Vec<u8>>,
    pub available_strategies_contract_names:
        BTreeMap<String, Vec<(ContractFileName, Option<BTreeMap<u64, ContractFileName>>)>>,
    pub selected_strategy: Option<String>,
    pub identity_asset_lock_private_key_in_creation: Option<(
        Vec<u8>,
        [u8; 32],
        Option<AssetLockProof>,
        Option<(Identity, BTreeMap<IdentityPublicKey, Vec<u8>>)>,
    )>,
    pub identity_asset_lock_private_key_in_top_up:
        Option<(Vec<u8>, [u8; 32], Option<AssetLockProof>)>,
}

impl PlatformSerializableWithPlatformVersion for AppState {
    type Error = ProtocolError;

    fn serialize_consume_to_bytes_with_platform_version(
        self,
        platform_version: &PlatformVersion,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.serialize_to_bytes_with_platform_version(&platform_version)
    }

    fn serialize_to_bytes_with_platform_version(
        &self,
        platform_version: &PlatformVersion,
    ) -> Result<Vec<u8>, ProtocolError> {
        let AppState {
            loaded_identity,
            identity_private_keys,
            loaded_wallet,
            drive,
            known_identities,
            known_contracts,
            supporting_contracts,
            available_strategies,
            selected_strategy,
            identity_asset_lock_private_key_in_creation,
            available_strategies_contract_names,
            identity_asset_lock_private_key_in_top_up,
        } = self;

        let known_contracts_in_serialization_format = known_contracts
            .blocking_lock()
            .iter()
            .map(|(key, contract)| {
                let serialized_contract =
                    contract.serialize_to_bytes_with_platform_version(platform_version)?;
                Ok((key.clone(), serialized_contract))
            })
            .collect::<Result<BTreeMap<String, Vec<u8>>, ProtocolError>>()?;

        let supporting_contracts_in_serialization_format = supporting_contracts
            .blocking_lock()
            .iter()
            .map(|(key, contract)| {
                let serialized_contract =
                    contract.serialize_to_bytes_with_platform_version(platform_version)?;
                Ok((key.clone(), serialized_contract))
            })
            .collect::<Result<BTreeMap<String, Vec<u8>>, ProtocolError>>()?;

        let available_strategies_in_serialization_format = available_strategies
            .blocking_lock()
            .iter()
            .map(|(key, strategy)| {
                let serialized_strategy =
                    strategy.serialize_to_bytes_with_platform_version(platform_version)?;
                Ok((key.clone(), serialized_strategy))
            })
            .collect::<Result<BTreeMap<String, Vec<u8>>, ProtocolError>>()?;

        let identity_asset_lock_private_key_in_creation =
            identity_asset_lock_private_key_in_creation
                .blocking_lock()
                .as_ref()
                .map(
                    |(transaction, private_key, asset_lock_proof, identity_info)| {
                        (
                            transaction.serialize(),
                            private_key.inner.secret_bytes(),
                            asset_lock_proof.clone(),
                            identity_info.clone(),
                        )
                    },
                );

        let identity_asset_lock_private_key_in_top_up = identity_asset_lock_private_key_in_top_up
            .blocking_lock()
            .as_ref()
            .map(|(transaction, private_key, asset_lock_proof)| {
                (
                    transaction.serialize(),
                    private_key.inner.secret_bytes(),
                    asset_lock_proof.clone(),
                )
            });

        let app_state_in_serialization_format = AppStateInSerializationFormat {
            loaded_identity: loaded_identity.blocking_lock().clone(),
            identity_private_keys: identity_private_keys.blocking_lock().clone(),
            loaded_wallet: loaded_wallet.blocking_lock().clone(),
            known_identities: known_identities.blocking_lock().clone(),
            known_contracts: known_contracts_in_serialization_format,
            supporting_contracts: supporting_contracts_in_serialization_format,
            available_strategies: available_strategies_in_serialization_format,
            selected_strategy: selected_strategy.blocking_lock().clone(),
            available_strategies_contract_names: available_strategies_contract_names
                .blocking_lock()
                .clone(),
            identity_asset_lock_private_key_in_creation,
            identity_asset_lock_private_key_in_top_up,
        };

        let config = bincode::config::standard()
            .with_big_endian()
            .with_no_limit();
        bincode::encode_to_vec(app_state_in_serialization_format, config).map_err(|e| {
            PlatformSerializationError(format!("unable to serialize App State: {}", e))
        })
    }
}

impl PlatformDeserializableWithPotentialValidationFromVersionedStructure for AppState {
    fn versioned_deserialize(
        data: &[u8],
        validate: bool,
        platform_version: &PlatformVersion,
    ) -> Result<Self, ProtocolError>
    where
        Self: Sized,
    {
        let config = bincode::config::standard()
            .with_big_endian()
            .with_no_limit();
        let app_state: AppStateInSerializationFormat =
            bincode::borrow_decode_from_slice(data, config)
                .map_err(|e| {
                    let msg = format!("Error decoding AppStateInSerializationFormat: {}", e);
                    PlatformDeserializationError(msg)
                })?
                .0;

        let AppStateInSerializationFormat {
            loaded_identity,
            identity_private_keys,
            loaded_wallet,
            known_identities,
            known_contracts,
            supporting_contracts,
            available_strategies,
            selected_strategy,
            available_strategies_contract_names,
            identity_asset_lock_private_key_in_creation,
            identity_asset_lock_private_key_in_top_up,
        } = app_state;

        let known_contracts = known_contracts
            .into_iter()
            .map(|(key, contract)| {
                let contract = DataContract::versioned_deserialize(
                    contract.as_slice(),
                    validate,
                    platform_version,
                )
                .map_err(|e| {
                    let msg = format!("Error deserializing known_contract for key {}: {}", key, e);
                    PlatformDeserializationError(msg)
                })?;
                Ok((key, contract))
            })
            .collect::<Result<BTreeMap<String, DataContract>, ProtocolError>>()?;

        let supporting_contracts = supporting_contracts
            .into_iter()
            .map(|(key, contract)| {
                let contract = DataContract::versioned_deserialize(
                    contract.as_slice(),
                    validate,
                    platform_version,
                )
                .map_err(|e| {
                    let msg = format!(
                        "Error deserializing supporting_contract for key {}: {}",
                        key, e
                    );
                    PlatformDeserializationError(msg)
                })?;
                Ok((key, contract))
            })
            .collect::<Result<BTreeMap<String, DataContract>, ProtocolError>>()?;

        let available_strategies = available_strategies
            .into_iter()
            .map(|(key, strategy)| {
                let strategy = Strategy::versioned_deserialize(
                    strategy.as_slice(),
                    validate,
                    platform_version,
                )
                .map_err(|e| {
                    let msg = format!(
                        "Error deserializing available_strategies for key {}: {}",
                        key, e
                    );
                    PlatformDeserializationError(msg)
                })?;
                Ok((key, strategy))
            })
            .collect::<Result<BTreeMap<String, Strategy>, ProtocolError>>()?;

        let identity_asset_lock_private_key_in_creation =
            identity_asset_lock_private_key_in_creation.map(
                |(transaction, private_key, asset_lock_proof, identity_info)| {
                    (
                        Transaction::deserialize(&transaction)
                            .expect("expected to deserialize transaction"),
                        // TODO: Should use network from config
                        PrivateKey::from_slice(&private_key, Network::Testnet)
                            .expect("expected private key"),
                        asset_lock_proof,
                        identity_info,
                    )
                },
            );

        let identity_asset_lock_private_key_in_top_up = identity_asset_lock_private_key_in_top_up
            .map(|(transaction, private_key, asset_lock_proof)| {
                (
                    Transaction::deserialize(&transaction)
                        .expect("expected to deserialize transaction"),
                    // TODO: Should use network from config
                    PrivateKey::from_slice(&private_key, Network::Testnet)
                        .expect("expected private key"),
                    asset_lock_proof,
                )
            });

        let (drive, _protocol_version) =
            Drive::open("explorer.drive", None).expect("expected to open Drive successfully");

        // Deserialize the wallet state and wrap it in Arc<Mutex<_>>
        let deserialized_wallet_state = loaded_wallet
            .map(|wallet| Mutex::new(Some(wallet)))
            .unwrap_or_else(|| Mutex::new(None));

        Ok(AppState {
            loaded_identity: loaded_identity.into(),
            identity_private_keys: identity_private_keys.into(),
            loaded_wallet: deserialized_wallet_state,
            drive: drive.into(),
            known_identities: known_identities.into(),
            known_contracts: known_contracts.into(),
            supporting_contracts: supporting_contracts.into(),
            available_strategies: available_strategies.into(),
            selected_strategy: selected_strategy.into(),
            available_strategies_contract_names: available_strategies_contract_names.into(),
            identity_asset_lock_private_key_in_creation:
                identity_asset_lock_private_key_in_creation.into(),
            identity_asset_lock_private_key_in_top_up: identity_asset_lock_private_key_in_top_up
                .into(),
        })
    }
}

impl AppState {
    pub async fn load(insight: &InsightAPIClient, config: &Config) -> AppState {
        let path = config.state_file_path();

        let Ok(read_result) = fs::read(path.clone()) else {
            let state = AppState::default();
            if let Some(private_key) = &config.wallet_private_key {
                let wallet_state = &state.loaded_wallet;
                add_wallet_by_private_key(&wallet_state, private_key).await;
            }
            return state;
        };

        let Ok(app_state) = AppState::versioned_deserialize(
            read_result.as_slice(),
            false,
            PlatformVersion::get(CURRENT_PROTOCOL_VERSION).unwrap(),
        ) else {
            let start = SystemTime::now();
            let since_the_epoch = start
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards");
            let timestamp = since_the_epoch.as_secs();

            let backup_path_str = format!("{}.{}.backup", path.display(), timestamp);
            let backup_path = PathBuf::from(backup_path_str);

            if let Err(e) = fs::rename(path, &backup_path) {
                eprintln!("Failed to backup old file: {}", e);
            }

            let state = AppState::default();
            if let Some(private_key) = &config.wallet_private_key {
                let wallet_state = &state.loaded_wallet;
                add_wallet_by_private_key(&wallet_state, private_key).await;
            }
            return state;
        };

        if let Some(private_key) = &config.wallet_private_key {
            let wallet_state = &app_state.loaded_wallet;
            add_wallet_by_private_key(&wallet_state, private_key).await;
        }

        // Load supporting contracts
        let platform_version = PlatformVersion::get(CURRENT_PROTOCOL_VERSION).unwrap();
        let mut supporting_contracts = BTreeMap::new();
        for entry in WalkDir::new("supporting_files/contract")
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        {
            let path = entry.path();
            let contract_name = path.file_stem().unwrap().to_str().unwrap().to_string();

            if let Ok(contract) = json_document_to_contract(&path, true, platform_version) {
                supporting_contracts.insert(contract_name, contract);
            }
        }

        {
            let mut app_state_supporting_contracts = app_state.supporting_contracts.lock().await;
            *app_state_supporting_contracts = supporting_contracts;
        }

        app_state
    }

    /// Used in backend destructor, must not panic
    pub fn save(&self, config: &Config) {
        let platform_version = PlatformVersion::get(CURRENT_PROTOCOL_VERSION).unwrap();
        let path = config.state_file_path();

        let serialized_state = tokio::task::block_in_place(|| {
            self.serialize_to_bytes_with_platform_version(platform_version)
        });
        if let Ok(state) = serialized_state {
            let _ = fs::write(path, state);
        }
    }
}
