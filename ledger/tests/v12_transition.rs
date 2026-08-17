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

//! v12→v13 transition, phase 3: the version-preserving envelope against genuine v12 bytes.
//!
//! `RAW_TX_V12` is a real pre-memo transaction extracted from this repository's own history —
//! not synthesized by the mirror types — so these tests exercise exactly the bytes an existing
//! Ledger9 chain would present. The strict single-version reader stays `[v13]`-only by design
//! (its clean rejection of v12 is pinned separately); historical decoding goes through
//! `prior_versions::versioned_deserialize`, which keeps the wire era explicit so a v12
//! transaction can never silently acquire a v13 header.

use ledger_storage_ledger_8::DefaultDB;
use midnight_node_ledger::ledger_9::tx_envelope::encode_as_prior_version;
use midnight_node_res::undeployed::transactions::{DEPLOY_TX, RAW_TX_V12};
use mn_ledger_9::prior_versions::{VersionedTransaction, versioned_deserialize};
use mn_ledger_9::structure::{ProofMarker, Signature};
use transient_crypto_ledger_9::commitment::PureGeneratorPedersen;

type Versioned = VersionedTransaction<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>;

fn decode(bytes: &[u8]) -> std::io::Result<Versioned> {
	versioned_deserialize::<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>(bytes)
}

/// The genuine v12 specimen decodes, stays labeled v12, reproduces its own bytes exactly, and
/// projects to a current-shaped transaction carrying no memos — the historical zero statement
/// element, unconditionally.
#[test]
fn genuine_v12_bytes_decode_with_version_preserved() {
	let versioned = decode(RAW_TX_V12).expect("a genuine v12 transaction must decode");
	assert!(versioned.is_v12(), "the wire era must be preserved, not normalized away");

	// Reserialization must reproduce the input byte-for-byte: accepted history stays history.
	let mut bytes = Vec::new();
	versioned.serialize_source(&mut bytes).expect("reserialization should succeed");
	assert_eq!(bytes, RAW_TX_V12, "v12 bytes must round-trip identically");

	// The projection for shared application logic is memo-less by construction.
	let latest = versioned.to_latest();
	assert!(
		latest.memo_records().is_empty(),
		"a v12 transaction must project with no memos: the old rules had none"
	);
}

/// A projected v12 transaction re-encoded as v13 is a *different* byte string: the projection
/// changes the wire era, which is exactly why reserialization must use the source variant.
#[test]
fn projection_to_v13_changes_the_encoding() {
	let versioned = decode(RAW_TX_V12).unwrap();
	let latest = versioned.to_latest();
	let mut v13_bytes = Vec::new();
	midnight_serialize::tagged_serialize(&latest, &mut v13_bytes).unwrap();
	assert_ne!(v13_bytes, RAW_TX_V12, "the eras have distinct encodings by design");
	assert!(v13_bytes.starts_with(b"midnight:transaction[v13]"));

	// And the v13 re-encoding decodes as v13 — the envelope reads both eras.
	let redecoded = decode(&v13_bytes).expect("the v13 re-encoding must decode");
	assert!(!redecoded.is_v12());
}

/// An unsupported version tag fails with a structured error naming the accepted tags — never a
/// panic, and never a partial value.
#[test]
fn unknown_version_tags_are_rejected_with_a_structured_error() {
	let mut forged = RAW_TX_V12.to_vec();
	// Rewrite the header's version digits: transaction[v12] -> transaction[v11].
	let header = b"midnight:transaction[v12]";
	assert!(forged.starts_with(header));
	forged[header.len() - 2] = b'1';
	let err = match decode(&forged) {
		Err(e) => e,
		Ok(_) => panic!("an unknown wire version must not decode"),
	};
	let msg = err.to_string();
	assert!(
		msg.contains("transaction[v12]") && msg.contains("transaction[v13]"),
		"the error must name both accepted tags, got: {msg}"
	);
}

/// A v13 header stapled onto a v12 body must fail: the body layouts differ by the memo
/// discriminant at every input, and cross-version hybrids are exactly what the plan forbids
/// accepting best-effort.
#[test]
fn v13_header_with_v12_body_is_rejected() {
	let v12_header = b"midnight:transaction[v12]";
	let mut hybrid = RAW_TX_V12.to_vec();
	assert!(hybrid.starts_with(v12_header));
	hybrid[v12_header.len() - 2] = b'1';
	hybrid[v12_header.len() - 1] = b'3';
	// Now claims to be transaction[v13] but carries the memo-less body.
	assert!(decode(&hybrid).is_err(), "a v13-header/v12-body hybrid must not decode");
}

/// Truncated v12 payloads fail cleanly.
#[test]
fn truncated_v12_bytes_are_rejected() {
	let cut = &RAW_TX_V12[..RAW_TX_V12.len() - 7];
	assert!(decode(cut).is_err());
}

/// One transaction, two encodings, one meaning.
///
/// `DEPLOY_TX` is a memo-less fixture that is valid against the undeployed genesis state, so it
/// can be written in either era. Re-encoding it as `transaction[v12]` and decoding that back
/// through the envelope must yield a transaction with the same identity as the v13 original —
/// which is what makes the node-side claim testable at all: whichever encoding arrives, the
/// ledger sees the same transaction. `pallet-midnight`'s `v12_and_v13_encodings_*` tests then
/// take these same two byte strings all the way through apply and compare state roots.
#[test]
fn the_two_encodings_of_one_transaction_decode_to_the_same_transaction() {
	let (v13_bytes, _block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
	let v12_bytes = encode_as_prior_version(&v13_bytes).expect("the fixture carries no memo");

	assert!(v12_bytes.starts_with(b"midnight:transaction[v12]"));
	assert_ne!(v12_bytes, v13_bytes, "the eras have distinct encodings by design");

	let as_v12 = decode(&v12_bytes).expect("the re-encoding must decode");
	let as_v13 = decode(&v13_bytes).expect("the fixture must decode");
	assert!(as_v12.is_v12());
	assert!(!as_v13.is_v12());

	assert_eq!(
		as_v12.to_latest().transaction_hash(),
		as_v13.to_latest().transaction_hash(),
		"the same transaction in either encoding must have one identity",
	);

	// And the v12 form still round-trips byte-identically, so nothing about being *derived* from
	// a v13 value makes it a second-class citizen at the boundary.
	let mut reserialized = Vec::new();
	as_v12
		.serialize_source(&mut reserialized)
		.expect("reserialization should succeed");
	assert_eq!(reserialized, v12_bytes);
}
