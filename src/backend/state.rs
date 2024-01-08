//! Application state module.
//! This kind of state does not include UI details and basically all about
//! persistence required by backend.

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
    util::deserializer::ProtocolVersion,
    version::PlatformVersion,
    ProtocolError,
    ProtocolError::{PlatformDeserializationError, PlatformSerializationError},
};
use strategy_tests::Strategy;
use tokio::sync::Mutex;
use walkdir::DirEntry;

use super::wallet::Wallet;
use crate::backend::insight::InsightAPIClient;
use crate::config::Config;

const CURRENT_PROTOCOL_VERSION: ProtocolVersion = 1;

const USE_LOCAL: bool = false;

pub(crate) type ContractFileName = String;

pub(super) type StrategiesMap = BTreeMap<String, Strategy>;
pub(crate) type StrategyContractNames =
    Vec<(ContractFileName, Option<BTreeMap<u64, ContractFileName>>)>;
pub(super) type KnownContractsMap = BTreeMap<String, DataContract>;

// TODO: each state part should be in it's own mutex in case multiple backend
// tasks are executed on different state parts,
// moreover single mutex hold during rendering will block unrelated tasks from
// finishing
#[derive(Debug)]
pub struct AppState {
    pub loaded_identity: Mutex<Option<Identity>>,
    pub identity_private_keys: Mutex<BTreeMap<(Identifier, KeyID), PrivateKey>>,
    pub loaded_wallet: Mutex<Option<Wallet>>,
    pub known_identities: Mutex<BTreeMap<Identifier, Identity>>,
    pub known_contracts: Mutex<KnownContractsMap>,
    pub available_strategies: Mutex<StrategiesMap>,
    /// Because we don't store which contract support file was used exactly we
    /// cannot properly restore the state and display a strategy, so this
    /// field serves as a double of strategies' `contracts_with_updates`,
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

// pub fn default_strategy_description(mut map: BTreeMap<String, String>) ->
// BTreeMap<String, String> {     map.insert("contracts_with_updates".
// to_string(), "".to_string());     map.insert("operations".to_string(),
// "".to_string());     map.insert("start_identities".to_string(),
// "".to_string());     map.insert("identities_inserts".to_string(),
// "".to_string());     map
// }

impl Default for AppState {
    fn default() -> Self {
        let mut known_contracts_raw = BTreeMap::new();
        // let mut available_strategies = BTreeMap::new();

        let _platform_version = PlatformVersion::get(CURRENT_PROTOCOL_VERSION).unwrap();

        fn is_json(entry: &DirEntry) -> bool {
            entry.path().extension().and_then(|s| s.to_str()) == Some("json")
        }

        // for entry in WalkDir::new("supporting_files/contract")
        //     .into_iter()
        //     .filter_map(|e| e.ok())
        //     .filter(is_json)
        // {
        //     let path = entry.path();
        //     let contract_name =
        // path.file_stem().unwrap().to_str().unwrap().to_string();
        //
        //     let contract = json_document_to_created_contract(&path, true,
        // platform_version)         .expect("expected to get contract from a
        // json document");
        //
        //     known_contracts_raw.insert(contract_name,
        // contract.data_contract_owned()); }

        let known_contracts = Mutex::from(known_contracts_raw);
        // let mut description1 = default_strategy_description(BTreeMap::new());
        // let mut description2 = default_strategy_description(BTreeMap::new());
        // let mut description3 = default_strategy_description(BTreeMap::new());
        // description1.insert("contracts_with_updates".to_string(),
        // "dashpay1".to_string()); description2.insert("contracts_with_updates"
        // .to_string(), "dashpay2".to_string()); description3.insert("
        // contracts_with_updates".to_string(), "dashpay3".to_string());
        //
        // let default_strategy_1 = Strategy {
        //     contracts_with_updates: vec![(
        //         known_contracts
        //             .get(&String::from("dashpay-contract-all-mutable"))
        //             .unwrap()
        //             .clone(),
        //         None,
        //     )],
        //     operations: vec![],
        //     start_identities: vec![],
        //     identities_inserts: Frequency {
        //         times_per_block_range: Default::default(),
        //         chance_per_block: None,
        //     },
        //     signer: None,
        // };
        // let default_strategy_2 = Strategy {
        //     contracts_with_updates: vec![(
        //         known_contracts
        //             .get(&String::from("dashpay-contract-all-mutable-update-1"))
        //             .unwrap()
        //             .clone(),
        //         None,
        //     )],
        //     operations: vec![],
        //     start_identities: vec![],
        //     identities_inserts: Frequency {
        //         times_per_block_range: Default::default(),
        //         chance_per_block: None,
        //     },
        //     signer: None,
        // };
        // let default_strategy_3 = Strategy {
        //     contracts_with_updates: vec![(
        //         known_contracts
        //             .get(&String::from("dashpay-contract-all-mutable-update-2"))
        //             .unwrap()
        //             .clone(),
        //         None,
        //     )],
        //     operations: vec![],
        //     start_identities: vec![],
        //     identities_inserts: Frequency {
        //         times_per_block_range: Default::default(),
        //         chance_per_block: None,
        //     },
        //     signer: None,
        // };
        //
        // available_strategies.insert(String::from("default_strategy_1"),
        // default_strategy_1); available_strategies.insert(String::from("
        // default_strategy_2"), default_strategy_2); available_strategies.
        // insert(String::from("default_strategy_3"), default_strategy_3);

        AppState {
            loaded_identity: None.into(),
            identity_private_keys: Default::default(),
            loaded_wallet: None.into(),
            known_identities: BTreeMap::new().into(),
            known_contracts,
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
    pub identity_private_keys: BTreeMap<(Identifier, KeyID), [u8; 32]>,
    pub loaded_wallet: Option<Wallet>,
    pub known_identities: BTreeMap<Identifier, Identity>,
    pub known_contracts: BTreeMap<String, Vec<u8>>,
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
            known_identities,
            known_contracts,
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

        let available_strategies_in_serialization_format = available_strategies
            .blocking_lock()
            .iter()
            .map(|(key, strategy)| {
                let serialized_strategy =
                    strategy.serialize_to_bytes_with_platform_version(platform_version)?;
                Ok((key.clone(), serialized_strategy))
            })
            .collect::<Result<BTreeMap<String, Vec<u8>>, ProtocolError>>()?;

        let identity_private_keys = identity_private_keys
            .blocking_lock()
            .iter()
            .map(|(key, value)| (key.clone(), value.inner.secret_bytes()))
            .collect();

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
            identity_private_keys,
            loaded_wallet: loaded_wallet.blocking_lock().clone(),
            known_identities: known_identities.blocking_lock().clone(),
            known_contracts: known_contracts_in_serialization_format,
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

        let identity_private_keys = identity_private_keys
            .into_iter()
            .map(|(key, value)| {
                (
                    key,
                    PrivateKey::from_slice(&value, Network::Testnet).expect("expected private key"), // TODO: Should use network from config
                )
            })
            .collect::<BTreeMap<(Identifier, u32), PrivateKey>>()
            .into();

        let identity_asset_lock_private_key_in_creation =
            identity_asset_lock_private_key_in_creation.map(
                |(transaction, private_key, asset_lock_proof, identity_info)| {
                    (
                        Transaction::deserialize(&transaction)
                            .expect("expected to deserialize transaction"),
                        PrivateKey::from_slice(&private_key, Network::Testnet)  // TODO: Should use network from config
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
                    PrivateKey::from_slice(&private_key, Network::Testnet)  // TODO: Should use network from config
                        .expect("expected private key"),
                    asset_lock_proof,
                )
            });

        Ok(AppState {
            loaded_identity: loaded_identity.into(),
            identity_private_keys,
            loaded_wallet: loaded_wallet.into(),
            known_identities: known_identities.into(),
            known_contracts: known_contracts.into(),
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

        let Ok(read_result) = fs::read(path) else {
            return AppState::default();
        };

        let Ok(app_state) = AppState::versioned_deserialize(
            read_result.as_slice(),
            false,
            PlatformVersion::get(CURRENT_PROTOCOL_VERSION).unwrap(),
        ) else {
            return AppState::default();
        };

        if let Some(wallet) = app_state.loaded_wallet.lock().await.as_mut() {
            wallet.reload_utxos(insight).await;
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
