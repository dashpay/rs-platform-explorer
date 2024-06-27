use std::time::SystemTimeError;

use dapi_grpc::tonic::Status;
use dpp::ProtocolError;
use rs_dapi_client::DapiClientError;

use crate::backend::{
    error::Error::{ParsingError, SdkError, WalletError},
    insight::InsightError,
    wallet,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("error while parsing an identity: {0}")]
    ParsingError(#[from] ProtocolError),
    #[error("ID encoding error: {0}")]
    Base58IdEncoding(#[from] bs58::decode::Error),
    #[error("System time error: {0}")]
    SystemTimeError(#[from] SystemTimeError),
    #[error("Wallet error: {0}")]
    WalletError(#[from] wallet::WalletError),
    #[error("SDK unexpected result: {0}")]
    SdkUnexpectedResultError(String),
    #[error("SDK error: {0} {1}")]
    SdkExplainedError(String, dash_sdk::Error),
    #[error("SDK error: {0}")]
    SdkError(#[from] dash_sdk::Error),
    #[error("Identity registration error: {0}")]
    IdentityRegistrationError(String),
    #[error("Identity top up error: {0}")]
    IdentityTopUpError(String),
    #[error("Identity withdrawal error: {0}")]
    IdentityWithdrawalError(String),
    #[error("Identity refresh error: {0}")]
    IdentityRefreshError(String),
    #[error("Identity error: {0}")]
    IdentityError(String),
    #[error("Document Signing error: {0}")]
    DocumentSigningError(String),
    #[error("DPNS error: {0}")]
    DPNSError(String),
}

impl From<dpp::platform_value::Error> for Error {
    fn from(value: dpp::platform_value::Error) -> Self {
        ParsingError(ProtocolError::ValueError(value))
    }
}

impl From<InsightError> for Error {
    fn from(value: InsightError) -> Self {
        WalletError(wallet::WalletError::Insight(value))
    }
}

impl From<DapiClientError<Status>> for Error {
    fn from(value: DapiClientError<Status>) -> Self {
        SdkError(value.into())
    }
}
