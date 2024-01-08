use std::time::SystemTimeError;

use dapi_grpc::tonic::Status;
use dpp::ProtocolError;
use rs_dapi_client::DapiClientError;

use crate::backend::{
    error::Error::{Parsing, Sdk, Wallet},
    insight::InsightError,
    wallet,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("error while parsing an identity: {0}")]
    Parsing(#[from] ProtocolError),
    #[error("ID encoding error: {0}")]
    Base58IdEncoding(#[from] bs58::decode::Error),
    #[error("System time error: {0}")]
    SystemTime(#[from] SystemTimeError),
    #[error("Wallet error: {0}")]
    Wallet(#[from] wallet::WalletError),
    #[error("SDK error: {0}: {1}")]
    SdkExplained(String, dash_platform_sdk::Error),
    #[error("SDK error: {0}")]
    Sdk(#[from] dash_platform_sdk::Error),
    #[error("Identity registration error: {0}")]
    IdentityRegistration(String),
    #[error("Identity top up error: {0}")]
    IdentityTopUp(String),
    #[error("Identity withdrawal error: {0}")]
    IdentityWithdrawal(String),
    #[error("Document Signing error: {0}")]
    DocumentSigning(String),
}

impl From<dpp::platform_value::Error> for Error {
    fn from(value: dpp::platform_value::Error) -> Self {
        Parsing(ProtocolError::ValueError(value))
    }
}

impl From<InsightError> for Error {
    fn from(value: InsightError) -> Self {
        Wallet(wallet::WalletError::Insight(value))
    }
}

impl From<DapiClientError<Status>> for Error {
    fn from(value: DapiClientError<Status>) -> Self {
        Sdk(value.into())
    }
}
