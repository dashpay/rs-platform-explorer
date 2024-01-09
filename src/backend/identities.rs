//! Identities backend logic.

use std::{cmp, collections::BTreeMap, time::Duration};

use dapi_grpc::{
    core::v0::{
        BroadcastTransactionRequest, GetStatusRequest, GetTransactionRequest,
        GetTransactionResponse,
    },
    platform::v0::{
        get_identity_balance_request, get_identity_balance_request::GetIdentityBalanceRequestV0,
        GetIdentityBalanceRequest,
    },
};
use dash_platform_sdk::{
    platform::{
        transition::{
            broadcast::BroadcastStateTransition, put_identity::PutIdentity,
            top_up_identity::TopUpIdentity, withdraw_from_identity::WithdrawFromIdentity,
        },
        Fetch,
    },
    Sdk,
};
use dpp::{
    bls_signatures::PrivateKey,
    dashcore::{psbt::serialize::Serialize, Address, Network, Transaction},
    identity::{
        accessors::{IdentityGettersV0, IdentitySettersV0},
        identity_public_key::{accessors::v0::IdentityPublicKeyGettersV0, v0::IdentityPublicKeyV0},
        KeyType, Purpose as KeyPurpose, Purpose, SecurityLevel as KeySecurityLevel, SecurityLevel,
    },
    platform_value::{string_encoding::Encoding, BinaryData, Identifier},
    prelude::{AssetLockProof, Identity, IdentityPublicKey},
    state_transition::{
        identity_update_transition::{
            methods::IdentityUpdateTransitionMethodsV0, v0::IdentityUpdateTransitionV0,
        },
        proof_result::StateTransitionProofResult,
        public_key_in_creation::v0::IdentityPublicKeyInCreationV0,
        StateTransition,
    },
    version::PlatformVersion,
};
use rand::{rngs::StdRng, SeedableRng};
use rs_dapi_client::{DapiRequestExecutor, RequestSettings};
use simple_signer::signer::SimpleSigner;
use tokio::sync::{MappedMutexGuard, MutexGuard};

use super::{state::IdentityPrivateKeysMap, AppStateUpdate, CompletedTaskPayload};
use crate::backend::{error::Error, stringify_result_keep_item, AppState, BackendEvent, Task};

pub(super) async fn fetch_identity_by_b58_id(
    sdk: &Sdk,
    base58_id: &str,
) -> Result<(Option<Identity>, String), String> {
    let id_bytes = Identifier::from_string(base58_id, Encoding::Base58)
        .map_err(|_| "can't parse identifier as base58 string".to_owned())?;

    let fetch_result = Identity::fetch(sdk, id_bytes).await;
    stringify_result_keep_item(fetch_result)
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum IdentityTask {
    RegisterIdentity(u64),
    TopUpIdentity(u64),
    WithdrawFromIdentity(u64),
    Refresh,
    CopyIdentityId,
    AddIdentityKey {
        key_type: KeyType,
        security_level: KeySecurityLevel,
        purpose: KeyPurpose,
    },
}

impl AppState {
    pub(crate) async fn run_identity_task(&self, sdk: &Sdk, task: IdentityTask) -> BackendEvent {
        match task {
            IdentityTask::RegisterIdentity(amount) => {
                let result = self.register_new_identity(sdk, amount).await;
                let execution_result = result
                    .as_ref()
                    .map(|_| "Executed successfully".into())
                    .map_err(|e| e.to_string());
                let app_state_update = match result {
                    Ok(identity) => AppStateUpdate::LoadedIdentity(identity),
                    Err(_) => AppStateUpdate::IdentityRegistrationProgressed,
                };

                BackendEvent::TaskCompletedStateChange {
                    task: Task::Identity(task),
                    execution_result,
                    app_state_update,
                }
            }
            IdentityTask::Refresh => {
                let result = self.refresh_identity(sdk).await;
                let execution_result = result
                    .as_ref()
                    .map(|_| "Executed successfully".into())
                    .map_err(|e| e.to_string());
                let app_state_update = match result {
                    Ok(identity) => AppStateUpdate::LoadedIdentity(identity),
                    Err(_) => AppStateUpdate::IdentityRegistrationProgressed,
                };

                BackendEvent::TaskCompletedStateChange {
                    task: Task::Identity(task),
                    execution_result,
                    app_state_update,
                }
            }
            IdentityTask::TopUpIdentity(amount) => {
                let result = self.top_up_identity(sdk, amount).await;
                let execution_result = result
                    .as_ref()
                    .map(|_| "Top up success".into())
                    .map_err(|e| e.to_string());
                match result {
                    Ok(identity) => BackendEvent::TaskCompletedStateChange {
                        task: Task::Identity(task),
                        execution_result,
                        app_state_update: AppStateUpdate::LoadedIdentity(identity),
                    },
                    Err(e) => BackendEvent::TaskCompleted {
                        task: Task::Identity(task),
                        execution_result: Err(e.to_string()),
                    },
                }
            }
            IdentityTask::WithdrawFromIdentity(amount) => {
                let result = self.withdraw_from_identity(sdk, amount).await;
                let execution_result = result
                    .as_ref()
                    .map(|_| "Successful withdrawal".into())
                    .map_err(|e| e.to_string());
                match result {
                    Ok(identity) => BackendEvent::TaskCompletedStateChange {
                        task: Task::Identity(task),
                        execution_result,
                        app_state_update: AppStateUpdate::LoadedIdentity(identity),
                    },
                    Err(e) => BackendEvent::TaskCompleted {
                        task: Task::Identity(task),
                        execution_result: Err(e.to_string()),
                    },
                }
            }
            IdentityTask::CopyIdentityId => {
                if let Some(loaded_identity) = self.loaded_identity.lock().await.as_ref() {
                    let id = loaded_identity.id();
                    cli_clipboard::set_contents(id.to_string(Encoding::Base58)).unwrap();
                    BackendEvent::TaskCompleted {
                        task: Task::Identity(task),
                        execution_result: Ok("Copied Identity Id".into()),
                    }
                } else {
                    BackendEvent::None
                }
            }
            IdentityTask::AddIdentityKey {
                key_type,
                security_level,
                purpose,
            } => {
                let loaded_identity_lock = self.loaded_identity.lock().await;
                let loaded_identity = if loaded_identity_lock.is_some() {
                    MutexGuard::map(loaded_identity_lock, |identity| {
                        identity.as_mut().expect("checked above")
                    })
                } else {
                    return BackendEvent::TaskCompleted {
                        task: Task::Identity(task),
                        execution_result: Err("No identity loaded".to_owned()),
                    };
                };

                let identity_private_keys_lock = self.identity_private_keys.lock().await;
                match add_identity_key(
                    sdk,
                    loaded_identity,
                    identity_private_keys_lock,
                    key_type,
                    security_level,
                    purpose,
                )
                .await
                {
                    Ok(app_state_update) => BackendEvent::TaskCompletedStateChange {
                        task: Task::Identity(task),
                        execution_result: Ok(CompletedTaskPayload::String(
                            "Successfully added a key to the identity".to_owned(),
                        )),
                        app_state_update,
                    },
                    Err(e) => BackendEvent::TaskCompleted {
                        task: Task::Identity(task),
                        execution_result: Err(e),
                    },
                }
            }
        }
    }

    pub(crate) async fn refresh_identity<'s>(
        &'s self,
        sdk: &Sdk,
    ) -> Result<MappedMutexGuard<'s, Identity>, Error> {
        let mut loaded_identity = self.loaded_identity.lock().await;

        if let Some(identity) = loaded_identity.as_ref() {
            let refreshed_identity = Identity::fetch(sdk, identity.id()).await?;
            if let Some(refreshed_identity) = refreshed_identity {
                loaded_identity.replace(refreshed_identity);
            }
        }
        let identity_result =
            MutexGuard::map(loaded_identity, |x| x.as_mut().expect("assigned above"));
        Ok(identity_result)
    }

    pub(crate) async fn refresh_identity_balance(&mut self, sdk: &Sdk) -> Result<(), Error> {
        if let Some(identity) = self.loaded_identity.blocking_lock().as_mut() {
            let balance = u64::fetch(
                sdk,
                GetIdentityBalanceRequest {
                    version: Some(get_identity_balance_request::Version::V0(
                        GetIdentityBalanceRequestV0 {
                            id: identity.id().to_vec(),
                            prove: true,
                        },
                    )),
                },
            )
            .await?;
            if let Some(balance) = balance {
                identity.set_balance(balance)
            }
        }
        Ok(())
    }

    pub(crate) async fn register_new_identity<'s>(
        &'s self,
        sdk: &Sdk,
        amount: u64,
    ) -> Result<MappedMutexGuard<'s, Identity>, Error> {
        // First we need to make the transaction from the wallet
        // We start by getting a lock on the wallet

        let mut loaded_wallet = self.loaded_wallet.lock().await;
        let Some(wallet) = loaded_wallet.as_mut() else {
            return Err(Error::IdentityRegistrationError(
                "No wallet loaded".to_string(),
            ));
        };

        //// Core steps

        let mut identity_asset_lock_private_key_in_creation = self
            .identity_asset_lock_private_key_in_creation
            .lock()
            .await;

        // We create the wallet registration transaction, this locks funds that we
        // can transfer from core to platform
        let (
            asset_lock_transaction,
            asset_lock_proof_private_key,
            maybe_asset_lock_proof,
            maybe_identity_info,
        ) = if let Some((
            asset_lock_transaction,
            asset_lock_proof_private_key,
            maybe_asset_lock_proof,
            maybe_identity,
        )) = identity_asset_lock_private_key_in_creation.as_ref()
        {
            (
                asset_lock_transaction.clone(),
                asset_lock_proof_private_key.clone(),
                maybe_asset_lock_proof.clone(),
                maybe_identity.clone(),
            )
        } else {
            let (asset_lock_transaction, asset_lock_proof_private_key) =
                wallet.registration_transaction(None, amount)?;

            identity_asset_lock_private_key_in_creation.replace((
                asset_lock_transaction.clone(),
                asset_lock_proof_private_key,
                None,
                None,
            ));

            (
                asset_lock_transaction,
                asset_lock_proof_private_key,
                None,
                None,
            )
        };

        let asset_lock_proof = if let Some(asset_lock_proof) = maybe_asset_lock_proof {
            asset_lock_proof.clone()
        } else {
            let asset_lock = Self::broadcast_and_retrieve_asset_lock(
                sdk,
                &asset_lock_transaction,
                &wallet.receive_address(),
            )
            .await
            .map_err(|e| {
                Error::SdkExplainedError("broadcasting transaction failed".to_string(), e)
            })?;

            identity_asset_lock_private_key_in_creation.replace((
                asset_lock_transaction.clone(),
                asset_lock_proof_private_key,
                Some(asset_lock.clone()),
                None,
            ));

            asset_lock
        };

        //// Platform steps

        let (identity, keys): (Identity, BTreeMap<IdentityPublicKey, Vec<u8>>) =
            if let Some(identity_info) = maybe_identity_info {
                identity_info.clone()
            } else {
                let mut std_rng = StdRng::from_entropy();
                let (mut identity, keys): (Identity, BTreeMap<IdentityPublicKey, Vec<u8>>) =
                    Identity::random_identity_with_main_keys_with_private_key(
                        2,
                        &mut std_rng,
                        sdk.version(),
                    )?;
                identity.set_id(
                    asset_lock_proof
                        .create_identifier()
                        .expect("expected to create an identifier"),
                );

                identity_asset_lock_private_key_in_creation.replace((
                    asset_lock_transaction.clone(),
                    asset_lock_proof_private_key,
                    Some(asset_lock_proof.clone()),
                    Some((identity.clone(), keys.clone())),
                ));

                (identity, keys)
            };

        let mut signer = SimpleSigner::default();

        signer.add_keys(keys);

        let updated_identity = identity
            .put_to_platform_and_wait_for_response(
                sdk,
                asset_lock_proof.clone(),
                &asset_lock_proof_private_key,
                &signer,
            )
            .await?;

        if updated_identity.id() != identity.id() {
            panic!("identity ids don't match");
        }

        let mut loaded_identity = self.loaded_identity.lock().await;

        loaded_identity.replace(updated_identity);
        let identity_result =
            MutexGuard::map(loaded_identity, |x| x.as_mut().expect("assigned above"));

        let keys = identity_asset_lock_private_key_in_creation
            .take()
            .expect("expected something to be in creation")
            .3
            .expect("expected an identity")
            .1
            .into_iter()
            .map(|(key, private_key)| ((identity.id(), key.id()), private_key));

        let mut identity_private_keys = self.identity_private_keys.lock().await;

        identity_private_keys.extend(keys);

        Ok(identity_result)
    }

    pub(crate) async fn top_up_identity<'s>(
        &'s self,
        sdk: &Sdk,
        amount: u64,
    ) -> Result<MappedMutexGuard<'s, Identity>, Error> {
        // First we need to make the transaction from the wallet
        // We start by getting a lock on the wallet

        let mut loaded_wallet = self.loaded_wallet.lock().await;
        let Some(wallet) = loaded_wallet.as_mut() else {
            return Err(Error::IdentityRegistrationError(
                "No wallet loaded".to_string(),
            ));
        };

        let mut identity_lock = self.loaded_identity.lock().await;

        let Some(identity) = identity_lock.as_mut() else {
            return Err(Error::IdentityTopUpError("No identity loaded".to_string()));
        };

        //// Core steps

        let mut identity_asset_lock_private_key_in_top_up =
            self.identity_asset_lock_private_key_in_top_up.lock().await;

        // We create the wallet registration transaction, this locks funds that we
        // can transfer from core to platform
        let (asset_lock_transaction, asset_lock_proof_private_key, maybe_asset_lock_proof) =
            if let Some((
                asset_lock_transaction,
                asset_lock_proof_private_key,
                maybe_asset_lock_proof,
            )) = identity_asset_lock_private_key_in_top_up.as_ref()
            {
                (
                    asset_lock_transaction.clone(),
                    asset_lock_proof_private_key.clone(),
                    maybe_asset_lock_proof.clone(),
                )
            } else {
                let (asset_lock_transaction, asset_lock_proof_private_key) =
                    wallet.registration_transaction(None, amount)?;

                identity_asset_lock_private_key_in_top_up.replace((
                    asset_lock_transaction.clone(),
                    asset_lock_proof_private_key,
                    None,
                ));

                (asset_lock_transaction, asset_lock_proof_private_key, None)
            };

        let asset_lock_proof = if let Some(asset_lock_proof) = maybe_asset_lock_proof {
            asset_lock_proof.clone()
        } else {
            let asset_lock = Self::broadcast_and_retrieve_asset_lock(
                sdk,
                &asset_lock_transaction,
                &wallet.receive_address(),
            )
            .await
            .map_err(|e| {
                Error::SdkExplainedError("error broadcasting transaction".to_string(), e)
            })?;

            identity_asset_lock_private_key_in_top_up.replace((
                asset_lock_transaction.clone(),
                asset_lock_proof_private_key,
                Some(asset_lock.clone()),
            ));

            asset_lock
        };

        //// Platform steps

        let updated_identity_balance: u64 = identity
            .top_up_identity(sdk, asset_lock_proof.clone(), &asset_lock_proof_private_key)
            .await?;

        identity.set_balance(updated_identity_balance);

        Ok(MutexGuard::map(identity_lock, |identity| {
            identity.as_mut().expect("checked above")
        })) // TODO too long above, better to refactor this one
    }

    pub(crate) async fn withdraw_from_identity<'s>(
        &'s self,
        sdk: &Sdk,
        amount: u64,
    ) -> Result<MappedMutexGuard<'s, Identity>, Error> {
        // First we need to make the transaction from the wallet
        // We start by getting a lock on the wallet

        let mut loaded_wallet = self.loaded_wallet.lock().await;
        let Some(wallet) = loaded_wallet.as_mut() else {
            return Err(Error::IdentityRegistrationError(
                "No wallet loaded".to_string(),
            ));
        };

        let new_receive_address = wallet.receive_address();

        let mut identity_lock = self.loaded_identity.lock().await;
        let Some(identity) = identity_lock.as_mut() else {
            return Err(Error::IdentityTopUpError("No identity loaded".to_string()));
        };

        let identity_public_key = identity
            .get_first_public_key_matching(
                Purpose::WITHDRAW,
                SecurityLevel::full_range().into(),
                KeyType::all_key_types().into(),
            )
            .ok_or(Error::IdentityWithdrawalError(
                "no withdrawal public key".to_string(),
            ))?;

        let loaded_identity_private_keys = self.identity_private_keys.lock().await;
        let Some(private_key) =
            loaded_identity_private_keys.get(&(identity.id(), identity_public_key.id()))
        else {
            return Err(Error::IdentityTopUpError(
                "No private key for withdrawal".to_string(),
            ));
        };

        let mut signer = SimpleSigner::default();

        signer.add_key(identity_public_key.clone(), private_key.to_vec());

        //// Platform steps

        let updated_identity_balance = identity
            .withdraw(sdk, new_receive_address, amount, None, signer)
            .await?;

        identity.set_balance(updated_identity_balance);

        Ok(MutexGuard::map(identity_lock, |identity| {
            identity.as_mut().expect("checked above")
        })) // TODO
    }

    pub(crate) async fn broadcast_and_retrieve_asset_lock(
        sdk: &Sdk,
        asset_lock_transaction: &Transaction,
        address: &Address,
    ) -> Result<AssetLockProof, dash_platform_sdk::Error> {
        let _span = tracing::debug_span!(
            "broadcast_and_retrieve_asset_lock",
            transaction_id = asset_lock_transaction.txid().to_string(),
        )
        .entered();

        let block_hash = sdk
            .execute(GetStatusRequest {}, RequestSettings::default())
            .await?
            .chain
            .map(|chain| chain.best_block_hash)
            .ok_or_else(|| {
                dash_platform_sdk::Error::DapiClientError("missing `chain` field".to_owned())
            })?;

        tracing::debug!(
            "starting the stream from the tip block hash {}",
            hex::encode(&block_hash)
        );

        let mut asset_lock_stream = sdk
            .start_instant_send_lock_stream(block_hash, address)
            .await?;

        tracing::debug!("stream is started");

        // we need to broadcast the transaction to core
        let request = BroadcastTransactionRequest {
            transaction: asset_lock_transaction.serialize(), /* transaction but how to encode it
                                                              * as bytes?, */
            allow_high_fees: false,
            bypass_limits: false,
        };

        tracing::debug!("broadcast the transaction");

        match sdk.execute(request, RequestSettings::default()).await {
            Ok(_) => tracing::debug!("transaction is successfully broadcasted"),
            Err(error) if error.to_string().contains("AlreadyExists") => {
                // Transaction is already broadcasted. We need to restart the stream from a
                // block when it was mined

                tracing::warn!("transaction is already broadcasted");

                let GetTransactionResponse { block_hash, .. } = sdk
                    .execute(
                        GetTransactionRequest {
                            id: asset_lock_transaction.txid().to_string(),
                        },
                        RequestSettings::default(),
                    )
                    .await?;

                tracing::debug!(
                    "restarting the stream from the transaction minded block hash {}",
                    hex::encode(&block_hash)
                );

                asset_lock_stream = sdk
                    .start_instant_send_lock_stream(block_hash, address)
                    .await?;

                tracing::debug!("stream is started");
            }
            Err(error) => {
                tracing::error!("transaction broadcast failed: {error}");

                return Err(error.into());
            }
        };

        tracing::debug!("waiting for asset lock proof");

        sdk.wait_for_asset_lock_proof_for_transaction(
            asset_lock_stream,
            asset_lock_transaction,
            Some(Duration::from_secs(4 * 60)),
        )
        .await
    }
}

async fn add_identity_key<'a>(
    sdk: &Sdk,
    mut loaded_identity: MappedMutexGuard<'a, Identity>,
    mut identity_private_keys: MutexGuard<'a, IdentityPrivateKeysMap>,
    key_type: KeyType,
    security_level: KeySecurityLevel,
    purpose: KeyPurpose,
) -> Result<AppStateUpdate<'a>, String> {
    let mut rng = StdRng::from_entropy();
    let platform_version = PlatformVersion::latest();

    let (public_key, private_key) = key_type
        .random_public_and_private_key_data(&mut rng, &platform_version)
        .map_err(|e| format!("Cannot generate key pair: {e}"))?;
    let identity_public_key: IdentityPublicKey = IdentityPublicKeyV0 {
        id: loaded_identity.get_public_key_max_id() + 1,
        purpose,
        security_level,
        contract_bounds: None,
        key_type,
        read_only: false,
        data: public_key.into(),
        disabled_at: None,
    }
    .into();

    let (master_public_key_id, master_public_key) = loaded_identity
        .public_keys()
        .iter()
        .find(|(_, key)| key.security_level() == KeySecurityLevel::MASTER)
        .ok_or_else(|| "No master key found for identity".to_owned())?;
    let master_private_key = identity_private_keys
        .get(&(loaded_identity.id(), *master_public_key_id))
        .ok_or_else(|| "Master private key not found".to_owned())?;

    let mut signer = SimpleSigner::default();
    signer.add_key(master_public_key.clone(), master_private_key.to_vec());
    signer.add_key(identity_public_key.clone(), private_key.clone());

    let mut identity_updated = loaded_identity.clone();
    identity_updated.bump_revision();

    let identity_update_transition = IdentityUpdateTransitionV0::try_from_identity_with_signer(
        &identity_updated,
        master_public_key_id,
        vec![Into::<IdentityPublicKeyInCreationV0>::into(identity_public_key.clone()).into()],
        Vec::new(),
        None,
        &signer,
        &platform_version,
        None,
    )
    .map_err(|e| format!("Unable to create state transition: {e}"))?;

    let StateTransitionProofResult::VerifiedIdentity(new_identity) = identity_update_transition
        .broadcast_and_wait(sdk, None)
        .await
        .map_err(|e| format!("Error broadcasting identity update transition: {e}"))?
    else {
        return Err(format!("Cannot verify identity update transition proof"));
    };

    *loaded_identity = new_identity;
    identity_private_keys.insert(
        (loaded_identity.id(), identity_public_key.id()),
        private_key,
    );

    Ok(AppStateUpdate::LoadedIdentity(loaded_identity))
}
