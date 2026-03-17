// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use async_graphql::Enum;
use async_graphql::SimpleObject;

/// An enum that specifies the intent scope for signature verification.
#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub(crate) enum IntentScope {
    /// Indicates that the bytes are to be parsed as transaction data bytes.
    TransactionData,
    /// Indicates that the bytes are to be parsed as a personal message.
    PersonalMessage,
}

/// The result of signature verification.
#[derive(SimpleObject, Clone, Debug)]
pub(crate) struct SignatureVerifyResult {
    /// Whether the signature was verified successfully.
    pub success: Option<bool>,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("Verification failed: {0}")]
    VerificationFailed(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}
