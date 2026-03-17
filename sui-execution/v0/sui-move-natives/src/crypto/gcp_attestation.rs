// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

// Stub implementation for old execution layers. The gcp_attestation Move module
// is included in the compiled framework so the native must be registered to pass
// bytecode verification. The enable_gcp_attestation feature flag is never set
// for this execution layer, so this native always returns NOT_SUPPORTED_ERROR.

use move_binary_format::errors::PartialVMResult;
use move_vm_runtime::native_functions::NativeContext;
use move_vm_types::{
    loaded_data::runtime_types::Type, natives::function::NativeResult, values::Value,
};
use std::collections::VecDeque;

pub const NOT_SUPPORTED_ERROR: u64 = 0;

pub fn verify_gcp_attestation_internal(
    context: &mut NativeContext,
    _ty_args: Vec<Type>,
    _args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    Ok(NativeResult::err(context.gas_used(), NOT_SUPPORTED_ERROR))
}
