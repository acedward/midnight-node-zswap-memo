// This file is part of midnight-node.
// Copyright (C) Midnight Foundation
// SPDX-License-Identifier: Apache-2.0
// Licensed under the Apache License, Version 2.0 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Transaction decoding for ledger generations that predate zswap input memos.
//!
//! These generations have exactly one transaction wire version, so decoding stays the strict
//! single-version read it has always been. Every transaction they can read is reported as
//! [`TxWireEra::PreMemo`] — truthfully, since a memo has no representation in their wire format
//! at all — so the shared activation gate never fires here. Their wire formats are also their
//! own: `transaction[v12]` of *ledger 9* is not a ledger-7 or ledger-8 transaction, and nothing
//! here tries to read one.

#![cfg(feature = "std")]

use super::{
	api::{self, DeserializableError},
	ledger_storage_local::db::DB,
	midnight_serialize_local::{Deserializable, Tagged},
	mn_ledger_local::structure::SignatureKind,
	types::LedgerApiError,
};
use crate::common::types::TxWireEra;

/// Decodes a submitted transaction, reporting which wire era it arrived in. See the ledger-9
/// counterpart in `ledger_9.rs`, which is where the two eras actually differ.
pub(crate) fn decode_versioned<S: SignatureKind<D>, D: DB>(
	api: &api::Api,
	bytes: &[u8],
) -> Result<(api::Transaction<S, D>, TxWireEra), LedgerApiError>
where
	api::Transaction<S, D>: Deserializable + DeserializableError + Tagged + 'static,
{
	api.tagged_deserialize::<api::Transaction<S, D>>(bytes)
		.map(|tx| (tx, TxWireEra::PreMemo))
}
