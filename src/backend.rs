//! Application backend.
//! This includes all logic unrelated to UI.

pub mod contracts;
pub mod documents;
pub mod error;
pub mod identities;
pub mod insight;
pub mod platform_info;
pub mod state;
pub mod strategies;
pub mod wallet;

use std::{
    collections::BTreeMap,
    fmt::{self, Display},
    time::Duration,
};

use dash_sdk::Sdk;
use dpp::{
    document::Document,
    identity::accessors::IdentityGettersV0,
    prelude::{Identifier, Identity},
    voting::vote_polls::contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll,
};
use drive_proof_verifier::types::{Contenders, ContestedResources};
use serde::Serialize;
pub(crate) use state::AppState;
use strategy_tests::Strategy;
use tokio::sync::{MappedMutexGuard, MutexGuard};

use self::state::KnownContractsMap;
pub(crate) use self::{
    contracts::ContractTask,
    state::StrategyContractNames,
    strategies::StrategyTask,
    wallet::{Wallet, WalletTask},
};
use crate::{
    backend::{
        documents::DocumentTask, identities::IdentityTask, insight::InsightAPIClient,
        platform_info::PlatformInfoTask, state::StrategiesMap,
    },
    config::Config,
};

/// Unit of work for the backend.
/// UI shall not execute any actions unrelated to rendering directly, to keep
/// things decoupled and for future UI/UX improvements it returns a [Task]
/// instead.
#[derive(Debug, Clone)]
pub enum Task {
    FetchIdentityById(String, bool),
    PlatformInfo(PlatformInfoTask),
    Strategy(StrategyTask),
    Wallet(WalletTask),
    Identity(IdentityTask),
    Contract(ContractTask),
    Document(DocumentTask),
}

/// A positive task execution result.
/// Occasionally it's desired to represent data on UI in a structured way, in
/// that case specific variants are used.
#[derive(Debug)]
pub enum CompletedTaskPayload {
    Documents(BTreeMap<Identifier, Option<Document>>),
    Document(Document),
    Identities(BTreeMap<Identifier, Identity>),
    String(String),
    ContestedResources(ContestedResources),
    ContestedResourceContenders(ContestedDocumentResourceVotePoll, Contenders),
}

impl From<String> for CompletedTaskPayload {
    fn from(value: String) -> Self {
        CompletedTaskPayload::String(value)
    }
}

impl From<&str> for CompletedTaskPayload {
    fn from(value: &str) -> Self {
        CompletedTaskPayload::String(value.to_owned())
    }
}

impl Display for CompletedTaskPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompletedTaskPayload::String(s) => write!(f, "{}", s),
            _ => write!(f, "Executed successfully"),
        }
    }
}

/// Any update coming from backend that UI may or may not react to.
#[derive(Debug)]
pub enum BackendEvent<'s> {
    TaskCompleted {
        task: Task,
        execution_result: Result<CompletedTaskPayload, String>,
    },
    TaskCompletedStateChange {
        task: Task,
        execution_result: Result<CompletedTaskPayload, String>,
        app_state_update: AppStateUpdate<'s>,
    },
    AppStateUpdated(AppStateUpdate<'s>),
    StrategyCompleted {
        strategy_name: String,
        result: StrategyCompletionResult,
    },
    StrategyError {
        error: String,
    },
    None,
}

/// Backend state update data on a specific field.
/// A screen implementation may handle specific updates to deliver a responsive
/// UI.
#[derive(Debug)]
pub(crate) enum AppStateUpdate<'s> {
    KnownContracts(MutexGuard<'s, KnownContractsMap>),
    LoadedWallet(MappedMutexGuard<'s, Wallet>),
    Strategies(
        MutexGuard<'s, StrategiesMap>,
        MutexGuard<'s, BTreeMap<String, StrategyContractNames>>,
    ),
    SelectedStrategy(
        String,
        MappedMutexGuard<'s, Strategy>,
        MappedMutexGuard<'s, StrategyContractNames>,
    ),
    IdentityRegistrationProgressed, // TODO provide state update details
    LoadedIdentity(MappedMutexGuard<'s, Identity>),
    LoadedEvonodeIdentity(MappedMutexGuard<'s, Identity>),
    FailedToRefreshIdentity,
    ClearedLoadedIdentity,
    ClearedLoadedWallet,
    IdentityCreditsTransferred,
    DPNSNameRegistered(String),
    DPNSNameRegistrationFailed,
}

/// Represents the result of completing a strategy.
#[derive(Debug)]
pub(crate) enum StrategyCompletionResult {
    Success {
        block_mode: bool,
        final_block_height: u64,
        start_block_height: u64,
        success_count: u64,
        transition_count: u64,
        run_time: Duration,
        init_time: Duration,
        rate: u64,
        success_rate: u64,
        success_percent: u64,
        dash_spent_identity: f64,
        dash_spent_wallet: f64,
    },
    PartiallyCompleted {
        reached_block_height: u64,
        reason: String,
    },
}

/// Application state, dependencies are task execution logic around it.
pub struct Backend<'a> {
    pub sdk: &'a Sdk,
    app_state: AppState,
    insight: InsightAPIClient,
    pub config: Config,
}

impl<'a> Backend<'a> {
    pub async fn new(sdk: &'a Sdk, insight: InsightAPIClient, config: Config) -> Backend<'a> {
        Backend {
            sdk,
            app_state: AppState::load(&insight, &config).await,
            insight,
            config,
        }
    }

    pub fn state(&self) -> &AppState {
        &self.app_state
    }

    pub async fn run_task(&self, task: Task) -> BackendEvent {
        match task {
            Task::FetchIdentityById(ref base58_id, add_to_known_identities) => {
                let execution_result =
                    identities::fetch_identity_by_b58_id(self.sdk, base58_id).await;
                if add_to_known_identities {
                    if let Ok((Some(identity), _)) = &execution_result {
                        let mut loaded_identities = self.app_state.known_identities.lock().await;
                        loaded_identities.insert(identity.id(), identity.clone());
                    }
                }

                let execution_info_result = execution_result
                    .map(|(_, result_info)| CompletedTaskPayload::String(result_info));

                BackendEvent::TaskCompleted {
                    task,
                    execution_result: execution_info_result,
                }
            }
            Task::Strategy(strategy_task) => {
                strategies::run_strategy_task(
                    &self.sdk,
                    &self.app_state,
                    strategy_task,
                    &self.insight,
                )
                .await
            }
            Task::Wallet(wallet_task) => {
                wallet::run_wallet_task(
                    self.sdk,
                    &self.app_state.loaded_wallet,
                    wallet_task,
                    &self.insight,
                )
                .await
            }
            Task::Contract(contract_task) => {
                contracts::run_contract_task(
                    self.sdk,
                    &self.app_state.known_contracts,
                    contract_task,
                )
                .await
            }
            Task::Identity(identity_task) => {
                self.app_state
                    .run_identity_task(self.sdk, identity_task)
                    .await
            }
            Task::Document(document_task) => {
                self.app_state
                    .run_document_task(&self.sdk, document_task)
                    .await
            }
            Task::PlatformInfo(platform_info_task) => {
                platform_info::run_platform_task(self.sdk, platform_info_task).await
            }
        }
    }
}

impl Drop for Backend<'_> {
    fn drop(&mut self) {
        self.app_state.save(&self.config)
    }
}

fn stringify_result<T: Serialize, E: Display>(result: Result<T, E>) -> Result<String, String> {
    match result {
        Ok(data) => Ok(as_json_string(&data)),
        Err(e) => Err(e.to_string()),
    }
}

fn stringify_result_keep_item<T: Serialize, E: Display>(
    result: Result<T, E>,
) -> Result<(T, String), String> {
    match result {
        Ok(data) => {
            let toml = as_json_string(&data);
            Ok((data, toml))
        }
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn as_toml<T: Serialize>(value: &T) -> String {
    toml::to_string_pretty(&value).unwrap_or("Cannot serialize as TOML".to_owned())
}

pub(crate) fn as_json_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(&value).unwrap_or("Cannot serialize as json string".to_owned())
}
