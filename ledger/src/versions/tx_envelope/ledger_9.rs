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

//! Ledger-9 transaction decoding, through the version-preserving v12/v13 envelope.
//!
//! Ledger 9 is the generation the memo upgrade splits in two: `transaction[v12]` is the encoding
//! every existing ledger-9 chain is written in, and `transaction[v13]` adds the per-input memo.
//! `mn_ledger_9::prior_versions::versioned_deserialize` reads both and reports which one it read;
//! this module keeps that answer and hands the caller the current-shaped projection to validate
//! and apply.
//!
//! The projection is memo-less for a v12 source by construction, which is byte-for-byte how the
//! old rules verified — so historical transactions are evaluated under historical rules without a
//! second rule set having to exist. What the projection must *not* do is replace the source:
//! reserializing accepted history goes through the envelope's own `serialize_source`, never
//! through the current type, so a v12 transaction can never silently acquire a v13 header. These
//! host paths never reserialize a submitted transaction — the node stores and gossips the bytes
//! it received — so handing on the projection here loses nothing.

#![cfg(feature = "std")]

use super::{
	LOG_TARGET, api,
	ledger_storage_local::db::DB,
	mn_ledger_local::{
		prior_versions::versioned_deserialize,
		structure::{ProofMarker, SignatureKind},
	},
	transient_crypto_local::commitment::PureGeneratorPedersen,
	types::{DeserializationError, LedgerApiError},
};
use crate::common::types::TxWireEra;

/// Re-encodes a memo-less transaction into the pre-memo `transaction[v12]` wire format.
///
/// Fixture and oracle support, not a node path: nothing in consensus ever writes a transaction
/// back out in the other era. What it makes possible is a test that pins the claim this whole
/// phase rests on — that the *same* transaction, presented in either encoding, reaches the same
/// verdict and leaves the same state root — because it is the only way to obtain a genuine v12
/// encoding of a transaction that also exists in v13 form.
///
/// A memo-bearing transaction has no v12 representation and is refused rather than stripped.
///
/// Fixed to the signature and database types the node actually runs with, so callers do not have
/// to name crate-internal type aliases.
#[cfg(feature = "test-utils")]
pub fn encode_as_prior_version(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
	use super::{
		TransactionSignature,
		ledger_storage_local::DefaultDB,
		midnight_serialize_local::{tagged_deserialize, tagged_serialize},
		mn_ledger_local::{prior_versions::TransactionV12, structure::Transaction},
	};

	type Current = Transaction<TransactionSignature, ProofMarker, PureGeneratorPedersen, DefaultDB>;
	type Prior =
		TransactionV12<TransactionSignature, ProofMarker, PureGeneratorPedersen, DefaultDB>;

	let current: Current = tagged_deserialize(bytes)?;
	let prior = Prior::try_from_current(&current)
		.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;

	let mut out = Vec::with_capacity(bytes.len());
	tagged_serialize(&prior, &mut out)?;
	Ok(out)
}

/// Decodes a submitted transaction, reporting which wire era it arrived in.
///
/// Each generation supplies its own version of this function under the same name — see the
/// ledger-7/8 counterpart in `pre_ledger_9.rs` — so the shared bridge code can decode without
/// naming ledger-9-only types that do not exist in earlier generations.
pub(crate) fn decode_versioned<S: SignatureKind<D>, D: DB>(
	_api: &api::Api,
	bytes: &[u8],
) -> Result<(api::Transaction<S, D>, TxWireEra), LedgerApiError> {
	let versioned = versioned_deserialize::<S, ProofMarker, PureGeneratorPedersen, D>(bytes)
		.map_err(|e| {
			// The envelope's error names both accepted tags, and separates an unknown version
			// from a truncated or non-canonical payload. Log it verbatim: the host API can only
			// hand the runtime back one deserialization code.
			log::error!(target: LOG_TARGET, "Error deserializing transaction: {e}");
			LedgerApiError::Deserialization(DeserializationError::Transaction)
		})?;

	let era = if versioned.is_v12() { TxWireEra::PreMemo } else { TxWireEra::MemoCapable };

	Ok((api::Transaction::new(versioned.to_latest()), era))
}
