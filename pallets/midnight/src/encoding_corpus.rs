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

//! The v12/v13 encoding-differential corpus (sub-01 phase 6).
//!
//! Phase 5 proved, for a single fixture, the claim this whole transition rests on: the same
//! transaction, presented in either wire encoding, reaches the same verdict and leaves the same
//! state roots. This module widens that assertion until the corpus is interesting.
//!
//! The corpus is a *designed* one — an ordered chain of scripted transactions built against the
//! `undeployed` genesis by `scripts/tests/generate-encoding-corpus.sh` and checked in under
//! `res/test-encoding-corpus/`, each entry proven against the genesis state plus every entry
//! before it. Replaying the chain from a fresh genesis is therefore exactly the situation each
//! transaction was built for. The harness replays it twice — once with every entry in its
//! `transaction[v13]` encoding, once with every dual-encodable entry re-encoded as
//! `transaction[v12]` — and compares, entry by entry, the verdict, the ledger state root and the
//! zswap state root.
//!
//! Why the corpus had to grow at all: the memo lives on a *zswap input*, and the entire v12↔v13
//! wire difference is one `Option` discriminant per input. Every fixture this repository had
//! before this phase carries zero zswap inputs, so for all of them the two encodings differ only
//! in the header string and are byte-for-byte the same length. The entries here carry one, three
//! and five-output shielded offers in both the guaranteed and the fallible position, so the
//! discriminant is actually on the wire — `multi-input-spend`'s v12 encoding is exactly three
//! bytes shorter than its v13 one, one per input.
//!
//! **What this is not.** There is no oracle here. No pre-upgrade reference binary is pinned and
//! no real-network history is replayed (owner decision, spec FR-018: this is a prototype
//! targeting fresh chains). Two runs producing identical reports is a *determinism* check; the
//! correctness claim it supports is the differential one — that the two encodings of one
//! transaction are indistinguishable in consensus state — and nothing wider. The report says so
//! in its own header, so it cannot be mistaken for external verification later.

// `ValidateUnsigned` is deprecated upstream but is still what this pallet implements; the rest of
// the test suite silences it the same way.
#![allow(deprecated)]

use crate::{
	Call as MidnightCall, mock,
	mock::{RuntimeOrigin, Test},
};
use frame_support::traits::OnFinalize;
use ledger_storage_ledger_8::DefaultDB;
use midnight_node_ledger::{
	ledger_9::tx_envelope::encode_as_prior_version,
	types::active_version::{BlockContext, DeserializationError},
};
use midnight_node_res::{
	encoding_corpus,
	networks::{MidnightNetwork, UndeployedNetwork},
};
use mn_ledger_9::{
	prior_versions::versioned_deserialize,
	structure::{ContractAction, ProofMarker, Signature, Transaction},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sp_runtime::{DispatchError, traits::ValidateUnsigned, transaction_validity::TransactionSource};
use transient_crypto_ledger_9::commitment::PureGeneratorPedersen;

type Tx = Transaction<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>;

// ----------------------------------------------------------------------------------------------
// Corpus manifest
// ----------------------------------------------------------------------------------------------

/// One entry of the checked-in corpus, as the manifest describes it.
struct Entry {
	name: String,
	kind: String,
	provenance: String,
	sha256: String,
	/// False for the one memo-bearing entry: a memo has no v12 representation at all, so that
	/// entry exists only in `transaction[v13]` form and is carried here as a companion.
	dual_encoded: bool,
	/// `chain` for an entry the next entry was built on top of; `companion` for one built
	/// against the chain but not part of it.
	role: String,
	/// The transaction as generated — always the current, memo-capable encoding.
	v13: Vec<u8>,
	block_context: BlockContext,
}

fn sha256_hex(bytes: &[u8]) -> String {
	let mut hasher = Sha256::new();
	hasher.update(bytes);
	hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn hex(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads the manifest, reverifies every fixture's hash, and returns the corpus in replay order.
///
/// The hash check is the point of the manifest, not decoration: the entries are proven against
/// each other, so a fixture that has drifted from the one its successors were built against
/// would fail somewhere far from the file that actually changed.
fn corpus() -> (Value, Vec<Entry>) {
	let manifest: Value =
		serde_json::from_slice(encoding_corpus::MANIFEST).expect("the corpus manifest must parse");
	let entries = manifest["entries"].as_array().expect("the manifest must list entries");

	let corpus = entries
		.iter()
		.map(|e| {
			let file = e["file"].as_str().expect("every entry names a file");
			let raw = encoding_corpus::entry(file)
				.unwrap_or_else(|| panic!("{file} is in the manifest but not embedded in midnight-node-res: add it to `encoding_corpus::ENTRIES`"));
			let expected = e["sha256"].as_str().expect("every entry carries a hash");
			assert_eq!(
				sha256_hex(raw),
				expected,
				"{file} does not match the hash the manifest pins for it",
			);

			let (v13, block_context) =
				midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(raw);
			Entry {
				name: e["name"].as_str().expect("name").to_string(),
				kind: e["kind"].as_str().expect("kind").to_string(),
				provenance: e["provenance"].as_str().expect("provenance").to_string(),
				sha256: expected.to_string(),
				dual_encoded: e["dual_encoded"].as_bool().expect("dual_encoded"),
				role: e["role"].as_str().expect("role").to_string(),
				v13,
				block_context: block_context.into(),
			}
		})
		.collect();

	(manifest, corpus)
}

/// What an entry actually contains, for the report — so a mismatch names not just the entry but
/// the shape that mismatched, and so the corpus can be *seen* to cover what it claims to.
fn shape(v13: &[u8]) -> Value {
	let versioned =
		versioned_deserialize::<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>(v13)
			.expect("every corpus entry must decode");
	let tx: Tx = versioned.to_latest();
	match &tx {
		Transaction::ClaimRewards(cr) => json!({
			"variant": "claim-rewards",
			"value": cr.value.to_string(),
			"kind": format!("{:?}", cr.kind),
		}),
		Transaction::Standard(st) => {
			let guaranteed = st.guaranteed_coins.as_ref().map(|o| {
				json!({
					"inputs": o.inputs.len(),
					"outputs": o.outputs.len(),
					"transients": o.transient.len(),
				})
			});
			let fallible = st
				.fallible_coins
				.iter()
				.map(|kv| {
					json!({
						"segment": *kv.0,
						"inputs": kv.1.inputs.len(),
						"outputs": kv.1.outputs.len(),
						"transients": kv.1.transient.len(),
					})
				})
				.collect::<Vec<_>>();
			let mut actions = Vec::new();
			let mut intents = Vec::new();
			for kv in st.intents.iter() {
				for action in Vec::from(&kv.1.actions) {
					actions.push(match action {
						ContractAction::Deploy(_) => "deploy",
						ContractAction::Call(_) => "call",
						ContractAction::Maintain(_) => "maintain",
					});
				}
				intents.push(json!({
					"segment": *kv.0,
					"guaranteed_unshielded_offer": kv.1.guaranteed_unshielded_offer.is_some(),
					"fallible_unshielded_offer": kv.1.fallible_unshielded_offer.is_some(),
					"dust_actions": kv.1.dust_actions.is_some(),
				}));
			}
			json!({
				"variant": "standard",
				"network_id": st.network_id,
				"guaranteed_zswap_offer": guaranteed,
				"fallible_zswap_offers": fallible,
				"contract_actions": actions,
				"intents": intents,
				"input_memos": tx.memo_records().len(),
			})
		},
	}
}

/// The transaction identity, which must survive re-encoding: the same transaction in either era
/// is the same transaction.
fn transaction_hash(bytes: &[u8]) -> String {
	let versioned =
		versioned_deserialize::<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>(bytes)
			.expect("must decode");
	format!("{:?}", versioned.to_latest().transaction_hash())
}

/// The v12 encoding of an entry, plus proof that it round-trips.
///
/// `versioned_deserialize` round-trip-checks every accepted v12 payload against its own
/// serializer before returning, so a successful decode already rejects non-canonical encodings;
/// this reserializes explicitly anyway, because "the harness reported byte-identical" is the
/// claim being made and it should not rest on a property held somewhere else.
fn v12_of(v13: &[u8]) -> Result<(Vec<u8>, bool), std::io::Error> {
	let v12 = encode_as_prior_version(v13)?;
	let decoded =
		versioned_deserialize::<Signature, ProofMarker, PureGeneratorPedersen, DefaultDB>(&v12)?;
	assert!(decoded.is_v12(), "a re-encoded transaction must read back as v12");
	let mut reserialized = Vec::new();
	decoded.serialize_source(&mut reserialized)?;
	Ok((v12.clone(), reserialized == v12))
}

// ----------------------------------------------------------------------------------------------
// Ledger harness
// ----------------------------------------------------------------------------------------------

fn init_ledger_state(block_context: &BlockContext) {
	let path_buf = tempfile::tempdir().unwrap().keep();
	let state_key = midnight_node_ledger::latest::storage::init_storage_paritydb_separate(
		&path_buf,
		UndeployedNetwork.genesis_state(),
		1024 * 1024,
	);
	sp_tracing::try_init_simple();
	mock::Midnight::initialize_state(UndeployedNetwork.id(), &state_key);
	mock::System::set_block_number(1);
	mock::Timestamp::set_timestamp(block_context.tblock * 1000);
}

/// Closes the current block and opens the next one at this entry's block context.
fn next_block(block_number: u64, block_context: &BlockContext) {
	<mock::Midnight as OnFinalize<u64>>::on_finalize(block_number);
	mock::System::set_block_number(block_number + 1);
	mock::Timestamp::set_timestamp(block_context.tblock * 1000);
}

fn apply(tx: Vec<u8>) -> Result<(), DispatchError> {
	mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx)
}

fn roots() -> (String, String) {
	(
		hex(&mock::Midnight::get_ledger_state_root().expect("ledger state root")),
		hex(&mock::Midnight::get_zswap_state_root().expect("zswap state root")),
	)
}

fn set_activation_height(height: u64) {
	crate::pallet::MemoActivationHeight::<Test>::put(height);
}

/// A verdict in a form two runs can be compared on: either "accepted" or the exact structured
/// error, never a free-form message.
fn verdict(result: Result<(), DispatchError>) -> String {
	match result {
		Ok(()) => "accepted".to_string(),
		Err(e) => format!("rejected: {e:?}"),
	}
}

/// Which encoding a replay presents each entry in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Era {
	/// The current, memo-capable encoding, as generated.
	V13,
	/// The pre-memo encoding, re-derived from the same transaction. Entries with no v12
	/// encoding (the memo-bearing companion) stay in their v13 form; the report says so.
	V12,
}

/// One entry's outcome in one replay.
#[derive(PartialEq, Eq, Debug)]
struct Outcome {
	encoding: &'static str,
	bytes: usize,
	sha256: String,
	verdict: String,
	ledger_root: String,
	zswap_root: String,
	events: usize,
}

/// Replays the whole corpus from a fresh genesis, one entry per block, in the requested encoding.
///
/// Activation is left at its default of 0 so the memo-capable encoding is live throughout: this
/// replay is about the encodings agreeing, not about the gate. The gate has its own matrix in
/// `tests.rs` and its block-level extension below.
fn replay(era: Era) -> Vec<Outcome> {
	let (_, entries) = corpus();

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(&entries[0].block_context);

		let mut outcomes = Vec::with_capacity(entries.len());
		for (index, entry) in entries.iter().enumerate() {
			if index > 0 {
				next_block(index as u64, &entry.block_context);
			}
			let events_before = mock::midnight_events().len();

			let (bytes, encoding) = match era {
				Era::V13 => (entry.v13.clone(), "v13"),
				Era::V12 if entry.dual_encoded => (
					v12_of(&entry.v13)
						.unwrap_or_else(|e| panic!("{}: v12 re-encoding failed: {e}", entry.name))
						.0,
					"v12",
				),
				// No v12 encoding exists for this one, by construction. Replaying the v13 bytes
				// keeps the chain intact and keeps the entry visible in both reports; the
				// `encoding` field is what stops that reading as a v12 result.
				Era::V12 => (entry.v13.clone(), "v13 (no v12 encoding exists)"),
			};

			let result = apply(bytes.clone());
			let (ledger_root, zswap_root) = roots();
			outcomes.push(Outcome {
				encoding,
				bytes: bytes.len(),
				sha256: sha256_hex(&bytes),
				verdict: verdict(result),
				ledger_root,
				zswap_root,
				events: mock::midnight_events().len() - events_before,
			});
		}
		outcomes
	})
}

// ----------------------------------------------------------------------------------------------
// Malformed / cross-version corpus (spec FR-015, FR-021; SC-008)
// ----------------------------------------------------------------------------------------------

/// The malformed corpus, derived from a real entry so each case differs from a *valid* payload in
/// exactly one way. `ledger/tests/v12_transition.rs` pins the same cases at the envelope level;
/// these run them through the node's transaction entry point, where a decode failure could still
/// mutate state or panic if the gate were in the wrong place.
fn malformed_cases(entry: &Entry) -> Vec<(&'static str, &'static str, Vec<u8>)> {
	let v13 = entry.v13.clone();
	let v12 = v12_of(&v13).expect("the base entry is dual-encodable").0;
	let v12_header = b"midnight:transaction[v12]";
	let v13_header = b"midnight:transaction[v13]";

	// The version digits sit two and three bytes from the end of `midnight:transaction[vNN]`, so
	// rewriting them leaves the closing bracket — and therefore a well-formed tag — in place. Get
	// this off by one and every case below degenerates into the same "unrecognised tag" test.
	let retag = |bytes: &[u8], to: &[u8; 2]| {
		let mut out = bytes.to_vec();
		out[v12_header.len() - 3] = to[0];
		out[v12_header.len() - 2] = to[1];
		assert!(out.starts_with(b"midnight:transaction[v"));
		assert_eq!(out[v12_header.len() - 1], b']');
		out
	};

	assert!(v12.starts_with(v12_header) && v13.starts_with(v13_header));

	vec![
		("unknown-version-tag", "a wire version that has never existed", retag(&v12, b"11")),
		("v13-header-v12-body", "the memo-capable header over a pre-memo body", retag(&v12, b"13")),
		("v12-header-v13-body", "the pre-memo header over a memo-capable body", retag(&v13, b"12")),
		("truncated-v12", "a pre-memo payload cut short", v12[..v12.len() - 7].to_vec()),
		("trailing-bytes-v12", "a complete pre-memo payload with junk appended", {
			let mut out = v12.clone();
			out.extend_from_slice(b"trailing");
			out
		}),
		("not-a-transaction", "bytes that are not a transaction at all", b"not a transaction".to_vec()),
		("empty", "no bytes at all", Vec::new()),
	]
}

// ----------------------------------------------------------------------------------------------
// The report
// ----------------------------------------------------------------------------------------------

/// Where the run's report goes. Under `target/` by default — never committed, and never inside
/// `res/`, which is checked in.
fn report_path() -> std::path::PathBuf {
	match std::env::var("ENCODING_DIFFERENTIAL_REPORT") {
		Ok(path) => path.into(),
		Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
			.join("../../target/encoding-differential-report.json"),
	}
}

/// The report carries no timestamps, paths, or run identifiers, so two clean runs of the same
/// corpus produce byte-identical files and the determinism check is a plain `cmp`.
fn write_report(report: &Value) {
	let path = report_path();
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).expect("report directory");
	}
	let rendered = serde_json::to_string_pretty(report).expect("the report must serialize");
	std::fs::write(&path, format!("{rendered}\n"))
		.unwrap_or_else(|e| panic!("failed writing {}: {e}", path.display()));
	println!("encoding-differential report written to {}", path.display());
}

// ----------------------------------------------------------------------------------------------
// The harness
// ----------------------------------------------------------------------------------------------

/// The whole phase-6 corpus in one run, emitting one report.
///
/// It is a single test on purpose, for two reasons. Its sections share the corpus and belong in
/// one report — "a mismatch names the entry" is only useful if there is exactly one artifact per
/// run to read it out of. And the ledger's storage arena is a *process*-global (`set_default_storage`
/// installs one for the whole test binary and every later caller silently reuses it), so several
/// allocation-heavy tests running concurrently in this module race on its reference counts; split
/// into three tests this harness failed intermittently inside the arena, with no fault of its own.
#[test]
fn the_encoding_differential_corpus_agrees_in_both_encodings() {
	let (manifest, entries) = corpus();

	// --- differential replay -------------------------------------------------------------
	let v13_run = replay(Era::V13);
	let v12_run = replay(Era::V12);
	assert_eq!(v13_run.len(), entries.len());
	assert_eq!(v12_run.len(), entries.len());

	let mut entry_reports = Vec::new();
	let mut failures = Vec::new();
	for (index, entry) in entries.iter().enumerate() {
		let (v13, v12) = (&v13_run[index], &v12_run[index]);

		let (v12_bytes, byte_identical) = match v12_of(&entry.v13) {
			Ok((bytes, identical)) => (Some(bytes), Some(identical)),
			Err(_) => (None, None),
		};
		let reserialization = match (entry.dual_encoded, byte_identical) {
			(true, Some(true)) => "byte-identical".to_string(),
			(true, Some(false)) => "NOT byte-identical".to_string(),
			(true, None) => "refused (unexpected for a dual-encoded entry)".to_string(),
			(false, None) => "no v12 encoding exists — refused, as it must be".to_string(),
			(false, Some(_)) => "a memo-bearing entry must NOT have a v12 encoding".to_string(),
		};

		let same_identity = v12_bytes
			.as_ref()
			.map(|bytes| transaction_hash(bytes) == transaction_hash(&entry.v13));

		let mut problems = Vec::new();
		if v13.verdict != v12.verdict {
			problems.push(format!("verdict differs: v13 {} vs v12 {}", v13.verdict, v12.verdict));
		}
		if v13.ledger_root != v12.ledger_root {
			problems.push("ledger state root differs".to_string());
		}
		if v13.zswap_root != v12.zswap_root {
			problems.push("zswap state root differs".to_string());
		}
		if v13.events != v12.events {
			problems.push(format!("event count differs: {} vs {}", v13.events, v12.events));
		}
		if entry.dual_encoded {
			if byte_identical != Some(true) {
				problems.push("v12 reserialization is not byte-identical".to_string());
			}
			if same_identity != Some(true) {
				problems.push("the two encodings do not share a transaction hash".to_string());
			}
		} else if v12_bytes.is_some() {
			problems.push("a memo-bearing entry must have no v12 encoding".to_string());
		}
		if !problems.is_empty() {
			failures.push(format!("{}: {}", entry.name, problems.join("; ")));
		}

		entry_reports.push(json!({
			"name": entry.name,
			"kind": entry.kind,
			"dual_encoded": entry.dual_encoded,
			"fixture_sha256": entry.sha256,
			"provenance": entry.provenance,
			"shape": shape(&entry.v13),
			"transaction_hash": transaction_hash(&entry.v13),
			"transaction_hash_matches_across_encodings": same_identity,
			"v12_reserialization": reserialization,
			"as_v13": json!({
				"encoding": v13.encoding,
				"bytes": v13.bytes,
				"sha256": v13.sha256,
				"verdict": v13.verdict,
				"ledger_state_root": v13.ledger_root,
				"zswap_state_root": v13.zswap_root,
				"events_emitted": v13.events,
			}),
			"as_v12": json!({
				"encoding": v12.encoding,
				"bytes": v12.bytes,
				"sha256": v12.sha256,
				"verdict": v12.verdict,
				"ledger_state_root": v12.ledger_root,
				"zswap_state_root": v12.zswap_root,
				"events_emitted": v12.events,
			}),
			"result": if problems.is_empty() { "pass" } else { "FAIL" },
			"problems": problems,
		}));
	}

	// The corpus is only worth running if it is not all one answer. Both of these are properties
	// of the *designed* corpus, so pin them here rather than trusting the generator stayed put.
	let accepted =
		entry_reports.iter().filter(|e| e["as_v13"]["verdict"] == "accepted").count();
	let rejected = entry_reports.len() - accepted;
	assert!(accepted >= 7, "the corpus must accept most of its entries, accepted {accepted}");
	assert!(rejected >= 1, "the corpus must contain a rejected entry, so the compared verdicts are not all the same");
	let with_inputs = entries
		.iter()
		.filter(|e| {
			let s = shape(&e.v13);
			s["guaranteed_zswap_offer"]["inputs"].as_u64().unwrap_or(0) > 0
				|| s["fallible_zswap_offers"]
					.as_array()
					.map(|a| a.iter().any(|o| o["inputs"].as_u64().unwrap_or(0) > 0))
					.unwrap_or(false)
		})
		.count();
	assert!(
		with_inputs >= 4,
		"the corpus must exercise zswap inputs — they are where the memo lives, and the only \
		 place the two encodings differ in the body — but only {with_inputs} entries have any",
	);

	// --- malformed and cross-version corpus ------------------------------------------------
	// The base must have zswap inputs. The two hybrid cases below retag one era's body with the
	// other's header, and the bodies only differ by the memo discriminant on an input — retag an
	// input-less transaction and you get a perfectly valid transaction of the other era, not a
	// hybrid. (`ledger/tests/v12_transition.rs` pins that fact separately.)
	let base = entries
		.iter()
		.find(|e| e.name == "multi-input-spend")
		.expect("the multi-input entry is the malformed corpus's base");
	assert!(
		shape(&base.v13)["guaranteed_zswap_offer"]["inputs"].as_u64().unwrap_or(0) > 0,
		"the malformed corpus needs a base with zswap inputs, or its hybrid cases are vacuous",
	);
	let malformed = malformed_corpus(base);
	failures.extend(
		malformed
			.iter()
			.filter(|c| c["result"] != "pass")
			.map(|c| format!("malformed/{}: {}", c["name"], c["problems"])),
	);

	// --- cross-boundary blocks ---------------------------------------------------------------
	let chain_verdicts: Vec<String> = entries
		.iter()
		.zip(v12_run.iter())
		.filter(|(entry, _)| entry.role == "chain")
		.map(|(_, outcome)| outcome.verdict.clone())
		.collect();
	let blocks = cross_boundary_blocks(&entries, &manifest, &chain_verdicts);
	failures.extend(
		blocks
			.iter()
			.filter(|c| c["result"] != "pass")
			.map(|c| format!("block/{}: {}", c["name"], c["problems"])),
	);

	// --- determinism, in process -------------------------------------------------------------
	// The weaker half of the determinism check: the same replay, twice, in one process. The real
	// one is two clean `cargo test` runs producing byte-identical report files, which is what
	// `scripts/tests/encoding-differential-determinism.sh` does. Neither is an oracle.
	let repeated = replay(Era::V13);
	if repeated != v13_run {
		failures.push("the v13 replay is not deterministic within one process".to_string());
	}
	let repeated_v12 = replay(Era::V12);
	if repeated_v12 != v12_run {
		failures.push("the v12 replay is not deterministic within one process".to_string());
	}

	// --- the memo-bearing companion ------------------------------------------------------------
	// A memo has no pre-memo representation, and the boundary must say so rather than quietly
	// dropping it — a stripped memo yields a transaction that still verifies, which is exactly how
	// an authenticated message would become an unauthenticated one. This is also why the rest of
	// the corpus is memo-less: there is nothing to compare a memo-bearing transaction against.
	let memo = entries.iter().find(|e| !e.dual_encoded).expect("the memo-bearing companion");
	if shape(&memo.v13)["input_memos"] != 1 {
		failures.push("the companion entry does not actually carry a memo".to_string());
	}
	let memo_refusal = match encode_as_prior_version(&memo.v13) {
		Ok(_) => {
			failures.push("a memo-bearing transaction must have no v12 encoding".to_string());
			"ACCEPTED — it must not be".to_string()
		},
		Err(e) => {
			if !e.to_string().contains("no v12 wire representation") {
				failures.push(format!("the memo refusal does not say why: {e}"));
			}
			e.to_string()
		},
	};

	let report = json!({
		"report": "v12/v13 encoding-differential corpus",
		"produced_by": "pallet-midnight test `the_encoding_differential_corpus_agrees_in_both_encodings`",
		"what_this_is": "A differential comparison between the two wire encodings of the same \
			transaction: each entry is applied through the node's transaction entry point once as \
			`transaction[v13]` and once as `transaction[v12]`, and the verdict, the ledger state \
			root and the zswap state root are compared.",
		"what_this_is_not": "This is not an oracle. No pre-upgrade reference binary is pinned and \
			no real-network history is replayed (owner decision, spec FR-018: this project is a \
			prototype targeting fresh chains). Running the harness twice and diffing the reports \
			is a DETERMINISM check, not external verification — it shows the harness returns the \
			same answers, not that those answers match a historically deployed system.",
		"corpus": {
			"source": manifest["corpus"],
			"genesis": manifest["genesis"],
			"network": manifest["network"],
			"generator": manifest["generator"],
			"ordering": manifest["note"],
		},
		"summary": {
			"entries": entry_reports.len(),
			"accepted_entries": accepted,
			"rejected_entries": rejected,
			"entries_with_zswap_inputs": with_inputs,
			"malformed_cases": malformed.len(),
			"cross_boundary_blocks": blocks.len(),
			"failures": failures.len(),
		},
		"determinism": {
			"what_was_checked": "the corpus replayed twice in this process, in both encodings, \
				reaching identical verdicts and identical state roots",
			"what_it_proves": "reproducibility of this harness — NOT agreement with any external \
				or historical system, of which none is pinned",
			"cross_run_check": "scripts/tests/encoding-differential-determinism.sh runs the \
				harness twice from clean state and requires byte-identical report files",
			"in_process_replays_agree": repeated == v13_run && repeated_v12 == v12_run,
		},
		"memo_bearing_companion": {
			"entry": memo.name,
			"why_it_is_not_dual_encoded": "a memo has no v12 wire representation, so the corpus \
				is memo-less by construction and this entry is carried as a v13-only companion",
			"v12_encoding_refused_with": memo_refusal,
		},
		"entries": entry_reports,
		"malformed": malformed,
		"cross_boundary_blocks": blocks,
	});
	write_report(&report);

	assert!(failures.is_empty(), "encoding-differential mismatches:\n  {}", failures.join("\n  "));
}

/// Every malformed, cross-version and unknown-version case, driven through the node's transaction
/// entry point rather than the envelope alone: what has to hold here is not only that decoding
/// fails, but that it fails *before* anything is written (spec FR-015, SC-008).
fn malformed_corpus(base: &Entry) -> Vec<Value> {
	let cases = malformed_cases(base);

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(&base.block_context);
		// Every case here is a *decoding* failure, and the host API can only hand the runtime one
		// deserialization code — the envelope's own error, which separates an unknown version
		// from a truncated or non-canonical payload, is logged rather than returned.
		let expected: DispatchError =
			crate::Error::<Test>::Deserialization(DeserializationError::Transaction).into();

		cases
			.into_iter()
			.map(|(name, description, bytes)| {
				let (ledger_before, zswap_before) = roots();
				let events_before = mock::midnight_events().len();
				let result = apply(bytes.clone());
				let (ledger_after, zswap_after) = roots();

				let mut problems = Vec::new();
				match &result {
					Ok(()) => problems.push("accepted a malformed transaction".to_string()),
					Err(e) if *e != expected => {
						problems.push(format!("unexpected error {e:?}"))
					},
					Err(_) => {},
				}
				if ledger_after != ledger_before {
					problems.push("mutated the ledger state".to_string());
				}
				if zswap_after != zswap_before {
					problems.push("mutated the zswap state".to_string());
				}
				if mock::midnight_events().len() != events_before {
					problems.push("emitted events".to_string());
				}

				json!({
					"name": name,
					"description": description,
					"bytes": bytes.len(),
					"sha256": sha256_hex(&bytes),
					"verdict": verdict(result),
					"state_unchanged": ledger_after == ledger_before && zswap_after == zswap_before,
					"result": if problems.is_empty() { "pass" } else { "FAIL" },
					"problems": problems,
				})
			})
			.collect()
	})
}

/// The activation matrix, lifted from single transactions to whole blocks.
///
/// Phase 5 pinned the boundary one transaction at a time. What a real boundary looks like is a
/// *block* holding both encodings, and the three positions a block can occupy relative to
/// activation. The corpus chain cannot supply that pair by itself: every chain entry was proven
/// against a state in which its predecessor had already been applied in an *earlier* block, so no
/// two of them fit in one block. The `cross-boundary-block` fixture exists for this — batch 0
/// funds two independent wallets, and batch 1's two transactions were both built against the same
/// parent state, which is exactly the relationship two transactions in one block have.
///
/// The chain is replayed in its pre-memo encoding, which is accepted at every height forever
/// (spec FR-011), so the prefix is in place no matter where the boundary is put.
fn cross_boundary_blocks(
	entries: &[Entry],
	manifest: &Value,
	expected_prefix: &[String],
) -> Vec<Value> {
	let (funding, pair) = mixed_block_fixture(manifest);
	let chain: Vec<&Entry> = entries.iter().filter(|e| e.role == "chain").collect();
	assert!(chain.iter().all(|e| e.dual_encoded), "the chain prefix must be re-encodable as v12");

	let not_active: DispatchError = crate::Error::<Test>::TransactionVersionNotActive.into();

	// `offset` is where the block under test sits relative to activation: -1 is the last block
	// before it, 0 is the boundary block itself, +1 the first block after.
	let run = |name: &'static str, description: &'static str, offset: i64, memo_active: bool| {
		mock::new_test_ext().execute_with(|| {
			init_ledger_state(&chain[0].block_context);
			// Out of reach while the prefix is applied, so the prefix is unambiguously running
			// under the pre-memo rules whatever the boundary is later set to.
			set_activation_height(u64::MAX);

			for (index, entry) in chain.iter().enumerate() {
				if index > 0 {
					next_block(index as u64, &entry.block_context);
				}
				let bytes = v12_of(&entry.v13).expect("chain entries are dual-encodable").0;
				// Not "must be accepted": the corpus deliberately contains an entry that is
				// rejected. What must hold is that replaying the chain under the pre-memo rules,
				// far from any activation height, reaches exactly the verdicts the differential
				// replay reached — otherwise the state this matrix runs against is not the state
				// the rest of the report describes.
				assert_eq!(
					verdict(apply(bytes)),
					expected_prefix[index],
					"{name}: the pre-memo prefix diverged at {}",
					entry.name,
				);
			}

			// The block under test: one block, both encodings. All three transactions of the
			// fixture go in it — batch 0 funds the two wallets, and batch 1's pair spends from
			// them. They belong together: dust is generated per unit of *time*, and these three
			// were proven against one another at a single instant, so splitting them across
			// blocks without advancing the clock invalidates the pair's dust spend proofs. A
			// block is exactly the granularity at which time does not advance.
			next_block(chain.len() as u64, &pair[1].1);
			let height = mock::System::block_number();
			let activation = (height as i64 - offset) as u64;
			set_activation_height(activation);

			let (ledger_before, zswap_before) = roots();

			let funding_result = apply(v12_of(&funding.0).expect("dual-encodable").0);
			let pre_memo_result = apply(v12_of(&pair[0].0).expect("dual-encodable").0);

			// The pool must agree with the block: what a block would reject must never be
			// admitted (spec FR-013), and what it accepts must be admissible. Asked *before* the
			// transaction is applied — afterwards it would be a replay, and the pool would refuse
			// it for a reason that has nothing to do with the boundary.
			let pool = <mock::Midnight as ValidateUnsigned>::validate_unsigned(
				TransactionSource::External,
				&MidnightCall::send_mn_transaction { midnight_tx: pair[1].0.clone() },
			);

			let memo_capable_result = apply(pair[1].0.clone());
			let (ledger_after, zswap_after) = roots();

			let mut problems = Vec::new();
			for (label, result) in
				[("the funding", &funding_result), ("the pre-memo", &pre_memo_result)]
			{
				if result.is_err() {
					problems.push(format!(
						"{label} transaction is pre-memo and must be accepted at every height, got {result:?}",
					));
				}
			}
			match (&memo_capable_result, memo_active) {
				(Ok(()), true) => {},
				(Err(e), false) if *e == not_active => {},
				(got, _) => problems.push(format!(
					"the memo-capable transaction was expected to be {}, got {got:?}",
					if memo_active { "accepted" } else { "refused as not-yet-active" },
				)),
			}
			if pool.is_ok() != memo_active {
				problems.push(format!("the pool disagreed with the block: {pool:?}"));
			}
			// The accepted pre-memo transactions moved value, so a block that changed nothing
			// would mean the fixture or the prefix had gone stale and the matrix was vacuous.
			if ledger_after == ledger_before && zswap_after == zswap_before {
				problems.push("the accepted transactions left the state untouched".to_string());
			}

			json!({
				"name": name,
				"description": description,
				"activation_height": activation,
				"block_height": height,
				"transactions_in_the_block": [
					{ "role": "funding", "encoding": "v12", "verdict": verdict(funding_result) },
					{ "role": "pre-memo", "encoding": "v12", "verdict": verdict(pre_memo_result) },
					{
						"role": "memo-capable",
						"encoding": "v13",
						"verdict": verdict(memo_capable_result),
					},
				],
				"pool_admits_memo_capable": pool.is_ok(),
				"ledger_state_root": ledger_after,
				"zswap_state_root": zswap_after,
				"result": if problems.is_empty() { "pass" } else { "FAIL" },
				"problems": problems,
			})
		})
	};

	vec![
		run(
			"last-block-before-activation",
			"one block below the boundary: the pre-memo transaction applies, the memo-capable one \
			 is refused as not-yet-active, and the refusal does not spoil the block",
			-1,
			false,
		),
		run(
			"exact-activation-block",
			"the boundary block itself, carrying both encodings: both apply",
			0,
			true,
		),
		run(
			"first-block-after-activation",
			"one block past the boundary, still carrying both encodings: the pre-memo encoding is \
			 accepted after activation too, indefinitely (spec FR-011)",
			1,
			true,
		),
	]
}

/// The cross-boundary block fixture, split into the funding transaction and the pair that shares
/// a block. Each transaction comes back with its own block context, the way the single-transaction
/// fixtures do.
type WithContext = (Vec<u8>, BlockContext);
fn mixed_block_fixture(manifest: &Value) -> (WithContext, Vec<WithContext>) {
	let meta = &manifest["cross_boundary_block"];
	let file = meta["file"].as_str().expect("file");
	let raw = encoding_corpus::entry(file).expect("the block fixture must be embedded");
	assert_eq!(
		sha256_hex(raw),
		meta["sha256"].as_str().expect("sha256"),
		"{file} does not match the hash the manifest pins for it",
	);

	let parsed: Value = serde_json::from_slice(raw).expect("the block fixture must parse");
	let batches = parsed["batches"].as_array().expect("the fixture is a batched file");
	assert_eq!(batches.len(), 2, "expected one funding batch and one co-block batch");

	// Each transaction is handed back through `extract_tx_with_context`, so the block context is
	// read exactly the way every other fixture in this repository reads it.
	let extract = |entry: &Value| -> WithContext {
			let single =
			json!({ "tx": entry["tx"], "context": entry["context"], "tx_hash": entry["tx_hash"] });
		let bytes = serde_json::to_vec(&single).expect("re-encoding one batch entry");
		let (tx, context) = midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(&bytes);
		(tx, context.into())
	};

	let funding = extract(&batches[0].as_array().expect("batch 0")[0]);
	let pair: Vec<WithContext> =
		batches[1].as_array().expect("batch 1").iter().map(extract).collect();
	assert_eq!(pair.len(), 2, "the co-block batch must hold exactly two transactions");
	(funding, pair)
}
