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
	// Rewrite the header's version digits: transaction[v12] -> transaction[v11]. The digits are
	// the two bytes before the closing bracket, which stays where it is.
	let header = b"midnight:transaction[v12]";
	assert!(forged.starts_with(header));
	forged[header.len() - 2] = b'1';
	assert!(forged.starts_with(b"midnight:transaction[v11]"));
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

/// A v13 header stapled onto a v12 body must fail — *when the bodies actually differ*.
///
/// They differ by one `Option` discriminant per zswap input, and by nothing else. So the check
/// needs a transaction that has inputs: `multi-input-spend` from the phase-6 corpus carries
/// three, and both hybrid directions are rejected for it.
#[test]
fn cross_version_hybrids_are_rejected_when_the_bodies_differ() {
	let (v13, _) = midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(
		midnight_node_res::encoding_corpus::entry("multi-input-spend.mn")
			.expect("the corpus entry must be embedded"),
	);
	let v12 = encode_as_prior_version(&v13).expect("the corpus entry carries no memo");
	assert_eq!(v12.len(), v13.len() - 3, "three inputs, three memo discriminants");

	// The version digits are the two bytes before the closing bracket; only the minor one moves,
	// so the tag stays well-formed and the decoder is really being asked about the *body*.
	let retag = |bytes: &[u8], digit: u8| {
		let mut out = bytes.to_vec();
		out[b"midnight:transaction[v12]".len() - 2] = digit;
		out
	};

	let v13_over_v12 = retag(&v12, b'3');
	assert!(v13_over_v12.starts_with(b"midnight:transaction[v13]"));
	assert!(decode(&v13_over_v12).is_err(), "a v13 header over a pre-memo body must not decode");

	let v12_over_v13 = retag(&v13, b'2');
	assert!(v12_over_v13.starts_with(b"midnight:transaction[v12]"));
	assert!(decode(&v12_over_v13).is_err(), "a v12 header over a memo-capable body must not decode");
}

/// The other half of that story, and the reason the test above needs a fixture with inputs: a
/// transaction with *no* zswap inputs has no memo discriminant anywhere, so its two encodings
/// differ only in the header string and are byte-for-byte the same length. Retagging one is not
/// forging a hybrid — it produces a genuine, well-formed encoding of the same transaction in the
/// other era, and the decoder is right to accept it.
///
/// `RAW_TX_V12`, the historical specimen, is exactly such a transaction. Worth pinning, because
/// "the tags disagree with the body" is only a detectable condition where the bodies can disagree.
#[test]
fn retagging_an_input_less_transaction_yields_a_valid_transaction_of_the_other_era() {
	let mut retagged = RAW_TX_V12.to_vec();
	retagged[b"midnight:transaction[v12]".len() - 2] = b'3';
	assert!(retagged.starts_with(b"midnight:transaction[v13]"));

	let decoded = decode(&retagged).expect(
		"an input-less v12 body is also a valid v13 body: there is no memo discriminant to differ",
	);
	assert!(!decoded.is_v12(), "it now reads as the era its header claims");
	assert_eq!(
		decoded.to_latest().transaction_hash(),
		decode(RAW_TX_V12).unwrap().to_latest().transaction_hash(),
		"and it is the same transaction, which is why accepting it is correct",
	);
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
