// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

// Stub implementation for old execution layers. The gcp_attestation Move module
// is included in the compiled framework so the native must be registered to pass
// bytecode verification. The feature is gated by enable_gcp_attestation(); since
// that flag is never set in these older execution layers, the native always
// returns NOT_SUPPORTED_ERROR.

use move_binary_format::errors::PartialVMResult;
use move_vm_runtime::native_functions::NativeContext;
use move_vm_types::{
    loaded_data::runtime_types::Type, natives::function::NativeResult, values::Value,
};
use std::collections::VecDeque;

use crate::object_runtime::ObjectRuntime;

pub const NOT_SUPPORTED_ERROR: u64 = 0;

pub fn verify_gcp_attestation_internal(
    context: &mut NativeContext,
    _ty_args: Vec<Type>,
    _args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let cost = context.gas_used();
    if !context
        .extensions()
        .get::<ObjectRuntime>()?
        .protocol_config
        .enable_gcp_attestation()
    {
        return Ok(NativeResult::err(cost, NOT_SUPPORTED_ERROR));
    }
    Ok(NativeResult::err(cost, NOT_SUPPORTED_ERROR))
}
