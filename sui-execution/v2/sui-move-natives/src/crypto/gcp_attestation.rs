// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

// Stub implementation for old execution layers. The gcp_attestation Move module
// is included in the compiled framework, so the native must be registered here
// to pass bytecode verification. However, the feature is only available in
// execution layer v4 (latest), so this stub always returns FEATURE_NOT_ENABLED.

use move_binary_format::errors::{PartialVMError, PartialVMResult};
use move_core_types::vm_status::StatusCode;
use move_vm_runtime::native_functions::NativeContext;
use move_vm_types::{
    loaded_data::runtime_types::Type, natives::function::NativeResult, values::Value,
};
use std::collections::VecDeque;

pub fn verify_gcp_attestation_internal(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    _args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    Err(
        PartialVMError::new(StatusCode::UNKNOWN_INVARIANT_VIOLATION_ERROR)
            .with_message("GCP attestation is not supported in this execution layer".to_string()),
    )
}
