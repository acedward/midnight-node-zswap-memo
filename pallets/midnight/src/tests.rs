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

// grcov-excl-start
#![allow(deprecated)]

use super::*;
use crate::{
	Call as MidnightCall, mock,
	mock::{RuntimeOrigin, Test},
};
use assert_matches::assert_matches;
use frame_support::{
	assert_err, assert_ok,
	pallet_prelude::Weight,
	traits::{OnFinalize, OnInitialize},
};
use frame_system::RawOrigin;
use midnight_node_ledger::types::active_version::{
	BlockContext, DeserializationError, LedgerApiError, MalformedError, TransactionError,
};
use midnight_node_res::{
	networks::{MidnightNetwork, UndeployedNetwork},
	undeployed::transactions::{
		CHECK_TX, CONTRACT_ADDR, DEPLOY_TX, MAINTENANCE_TX, STORE_TX, ZSWAP_TX,
	},
};
use midnight_primitives_ledger::{TBlockCorrection, TBlockCorrectionExt};
use sp_runtime::{
	traits::ValidateUnsigned,
	transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
};
use test_log::test;

fn init_ledger_state(block_context: BlockContext) {
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

fn process_block(block_number: u64, block_context: BlockContext) {
	mock::Midnight::on_finalize(block_number);
	mock::System::set_block_number(block_number + 1);
	mock::Timestamp::set_timestamp(block_context.tblock * 1000);
}

/// Drives the start of a block the way a real one is built: `on_initialize` records the parent
/// block's timestamp (which becomes the ledger's `last_block_time`) while `pallet_timestamp`
/// still holds it, and only then does the timestamp inherent set the block's own.
fn begin_block(block_number: u64, parent_ts: u64, block_ts: u64) {
	mock::System::set_block_number(block_number);
	mock::Timestamp::set_timestamp(parent_ts * 1000);
	<mock::Midnight as OnInitialize<u64>>::on_initialize(block_number);
	mock::Timestamp::set_timestamp(block_ts * 1000);
}

/// A correction with a zero offset, which isolates what the correction is measured *from*: the
/// corrected timestamp becomes exactly the parent block's, which is the timestamp the producing
/// node's warm strict-cache entry was verified at. The offset arithmetic on top of that base is
/// covered by the `well_formed_tblock` unit tests in `midnight-node-ledger`.
fn parent_base_correction(disable_after: u64) -> TBlockCorrection {
	TBlockCorrection { offset: 0, disable_after }
}

fn ext_with_correction(correction: Option<TBlockCorrection>) -> sp_io::TestExternalities {
	let mut ext = mock::new_test_ext();
	if let Some(correction) = correction {
		ext.register_extension(TBlockCorrectionExt(correction));
	}
	ext
}

fn send_mn_transaction(tx: Vec<u8>) -> sp_runtime::DispatchResult {
	mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx)
}

/// A block timestamp shortly after `block_context`'s, distinct on every call.
///
/// The strict transaction-validation cache is a process-global static keyed by
/// `(ledger state hash, tx hash, block timestamp)`, and every `tblock_correction` test below
/// validates the same fixture against the same genesis state. Handing out a distinct block
/// timestamp per assertion keeps them from serving each other's cached `well_formed` results —
/// which would make an assertion pass without ever running the code it is testing. The
/// timestamps stay well inside the fixture's validity window.
fn uncached_block_ts(block_context: &BlockContext) -> u64 {
	static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(101);
	block_context.tblock + NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn test_send_mn_transaction() {
	mock::new_test_ext().execute_with(|| {
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
		init_ledger_state(block_context.into());

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx));

		// Check emitted events
		let events = mock::midnight_events();
		assert_matches!(events[0], Event::ContractDeploy(_));
		assert_matches!(events[1], Event::TxApplied(_));
	})
}

#[test]
fn test_send_mn_transaction_malformed_tx() {
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		let bytes = vec![1, 2, 3];
		let error: sp_runtime::DispatchError =
			Error::<Test>::Deserialization(DeserializationError::Transaction).into();
		assert_err!(
			mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), bytes.clone()),
			error
		);

		// Check emitted events
		assert!(mock::midnight_events().is_empty());
	})
}

#[test]
fn test_send_mn_transaction_invalid_tx() {
	mock::new_test_ext().execute_with(|| {
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(STORE_TX);
		init_ledger_state(block_context.into());

		let error: sp_runtime::DispatchError = Error::<Test>::Transaction(
			TransactionError::Malformed(MalformedError::ContractNotPresent),
		)
		.into();
		assert_err!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx), error);

		// Check emitted events
		assert!(mock::midnight_events().is_empty());
	})
}

#[test]
fn test_get_contract_state() {
	mock::new_test_ext().execute_with(|| {
		let (tx_deploy, block_context_deploy) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
		let (tx_store, block_context_store) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(STORE_TX);
		let (tx_check, block_context_check) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(CHECK_TX);
		let (tx_maintenance, block_context_maintenance) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(MAINTENANCE_TX);

		init_ledger_state(block_context_deploy.into());

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx_deploy));
		process_block(2, block_context_store.into());

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx_store));
		process_block(3, block_context_check.into());

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx_check));
		process_block(4, block_context_maintenance.into());

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx_maintenance));

		let addr = hex::decode(CONTRACT_ADDR).expect("Address should be a valid hex code");

		let result = mock::Midnight::get_contract_state(&addr);
		assert!(result.is_ok(), "Failed calling `get_contract_state`");
	})
}

#[test]
fn test_get_contract_state_not_present() {
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		// A random address that hasn't been deployed
		let addr = hex::decode(CONTRACT_ADDR).expect("Address should be a valid hex code");

		let result = mock::Midnight::get_contract_state(&addr);
		assert_eq!(result, Err(LedgerApiError::ContractNotPresent));
	})
}

#[test]
fn test_get_unclaimed_amount_beneficiary_not_found() {
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		// A 32-byte address that has never had any unclaimed rewards in the genesis state
		let addr = [0u8; 32];
		let result = mock::Midnight::get_unclaimed_amount(&addr);
		assert_eq!(result, Err(LedgerApiError::BeneficiaryNotFound));
	})
}

#[test]
fn test_validation_works() {
	let (tx, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);

	let call = MidnightCall::send_mn_transaction { midnight_tx: tx };
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context.into());

		assert_ok!(<mock::Midnight as ValidateUnsigned>::validate_unsigned(
			TransactionSource::External,
			&call
		));
	})
}

#[test]
fn test_validation_fails() {
	let call = MidnightCall::send_mn_transaction { midnight_tx: vec![1, 2, 3] };

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		assert_err!(
			<mock::Midnight as ValidateUnsigned>::validate_unsigned(
				TransactionSource::External,
				&call
			),
			//todo here
			TransactionValidityError::Invalid(InvalidTransaction::Custom(
				LedgerApiError::Deserialization(DeserializationError::Transaction).into()
			))
		);
	});
}

#[test]
fn test_pre_dispatch_accepts_valid_transaction() {
	let (tx, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);

	let call = MidnightCall::send_mn_transaction { midnight_tx: tx };
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context.into());

		// pre_dispatch should succeed for a valid transaction
		assert_ok!(<mock::Midnight as ValidateUnsigned>::pre_dispatch(&call));
	})
}

#[test]
fn test_pre_dispatch_rejects_contract_not_present() {
	// STORE_TX requires a deployed contract, so without DEPLOY_TX it will fail
	// This tests the DDoS mitigation: transactions that would fail the guaranteed
	// part are rejected at pre_dispatch time, before consuming blockspace.
	let (tx, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(STORE_TX);

	let call = MidnightCall::send_mn_transaction { midnight_tx: tx };
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context.into());
		// Note: DEPLOY_TX not applied - contract doesn't exist

		// pre_dispatch should fail because the contract doesn't exist
		let result = <mock::Midnight as ValidateUnsigned>::pre_dispatch(&call);
		assert!(
			result.is_err(),
			"pre_dispatch should reject transaction with missing contract dependency"
		);
	});
}

#[test]
fn test_pre_dispatch_rejects_malformed_transaction() {
	let call = MidnightCall::send_mn_transaction { midnight_tx: vec![1, 2, 3] };

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		// pre_dispatch should fail for malformed transaction
		assert_err!(
			<mock::Midnight as ValidateUnsigned>::pre_dispatch(&call),
			TransactionValidityError::Invalid(InvalidTransaction::Custom(
				LedgerApiError::Deserialization(DeserializationError::Transaction).into()
			))
		);
	});
}

/// PR367-TC-0003-02: ReplayProtection Rejection
/// Verify that a replayed transaction is rejected at `pre_dispatch`.
#[test]
fn test_pre_dispatch_rejects_replay_attack() {
	mock::new_test_ext().execute_with(|| {
		// Set up ledger state and deploy contract
		let (deploy_tx, block_context_deploy) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
		let (store_tx, block_context_store) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(STORE_TX);

		init_ledger_state(block_context_deploy.into());

		// Step 1: Deploy the contract
		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), deploy_tx));

		// Process block and advance to store transaction context
		process_block(2, block_context_store.into());

		// Step 2: Apply STORE_TX successfully via the pallet (not pre_dispatch)
		let store_tx_clone = store_tx.clone();
		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), store_tx));

		// Step 3: Try to replay the same STORE_TX via pre_dispatch
		// This should fail because the replay protection counter has been consumed
		let call = MidnightCall::send_mn_transaction { midnight_tx: store_tx_clone };
		let result = <mock::Midnight as ValidateUnsigned>::pre_dispatch(&call);

		// pre_dispatch should reject the replay attempt
		assert!(result.is_err(), "pre_dispatch should reject replayed transaction");
	});
}

/// PR367-TC-0003-05: Validation Does Not Modify State
/// Verify that `validate_guaranteed_execution` (via pre_dispatch) is read-only.
#[test]
fn test_pre_dispatch_validation_does_not_modify_state() {
	mock::new_test_ext().execute_with(|| {
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);

		init_ledger_state(block_context.into());

		// Record state before validation
		let state_root_before =
			mock::Midnight::get_zswap_state_root().expect("Should be able to get state root");

		// Create call and run pre_dispatch
		let call = MidnightCall::send_mn_transaction { midnight_tx: tx };
		let _result = <mock::Midnight as ValidateUnsigned>::pre_dispatch(&call);

		// Record state after validation
		let state_root_after =
			mock::Midnight::get_zswap_state_root().expect("Should be able to get state root");

		// State should be unchanged - validation must be read-only
		assert_eq!(
			state_root_before, state_root_after,
			"State should not be modified by pre_dispatch validation"
		);
	});
}

/// PR367-TC-0003-05 (variant): Verify validation doesn't modify state even for failing validation
#[test]
fn test_pre_dispatch_validation_does_not_modify_state_on_failure() {
	mock::new_test_ext().execute_with(|| {
		// STORE_TX will fail (no contract deployed) but should not modify state
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(STORE_TX);

		init_ledger_state(block_context.into());

		// Record state before validation
		let state_root_before =
			mock::Midnight::get_zswap_state_root().expect("Should be able to get state root");

		// Create call and run pre_dispatch (will fail)
		let call = MidnightCall::send_mn_transaction { midnight_tx: tx };
		let result = <mock::Midnight as ValidateUnsigned>::pre_dispatch(&call);
		assert!(result.is_err(), "pre_dispatch should fail for missing contract");

		// Record state after validation
		let state_root_after =
			mock::Midnight::get_zswap_state_root().expect("Should be able to get state root");

		// State should be unchanged - even failed validation must be read-only
		assert_eq!(
			state_root_before, state_root_after,
			"State should not be modified by failed pre_dispatch validation"
		);
	});
}

#[test]
fn sets_extra_transaction_size_weight() {
	mock::new_test_ext().execute_with(|| {
		let before_weight = mock::Midnight::configurable_transaction_size_weight();

		assert_eq!(before_weight, crate::EXTRA_WEIGHT_TX_SIZE);

		let new_weight = Weight::from_parts(42, 0);

		mock::Midnight::set_tx_size_weight(RawOrigin::Root.into(), new_weight).unwrap();

		let after_weight = mock::Midnight::configurable_transaction_size_weight();

		assert_eq!(after_weight, new_weight);
	});
}

#[test]
fn test_get_mn_transaction_fee() {
	mock::new_test_ext().execute_with(|| {
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);

		init_ledger_state(block_context.into());

		let gas_cost = mock::Midnight::get_transaction_cost(&tx).unwrap();

		// Assert the transaction has some associated cost
		assert!(gas_cost > 0);
	});
}

#[test]
fn test_get_ledger_parameters() {
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		let parameters = mock::Midnight::get_ledger_parameters();

		assert_ok!(parameters);
	});
}

#[test]
#[ignore = "Cannot update ZSWAP_TX because we have no test tokens in genesis"]
fn test_send_zswap_tx() {
	mock::new_test_ext().execute_with(|| {
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(ZSWAP_TX);

		init_ledger_state(block_context.into());

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx));
	});
}

#[test]
#[ignore = "Cannot update ZSWAP_TX because we have no test tokens in genesis"]
fn test_get_zswap_state_root() {
	mock::new_test_ext().execute_with(|| {
		let (tx, block_context) =
			midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(ZSWAP_TX);

		init_ledger_state(block_context.into());

		let root = mock::Midnight::get_zswap_state_root().unwrap();

		assert_ok!(mock::Midnight::send_mn_transaction(RuntimeOrigin::none(), tx));

		mock::System::set_block_number(2);

		let new_root = mock::Midnight::get_zswap_state_root().unwrap();

		assert_ne!(new_root, root);
	});
}

#[test]
fn test_get_ledger_state_root() {
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		let root = mock::Midnight::get_ledger_state_root();

		assert_ok!(&root);
		assert!(!root.unwrap().is_empty());
	});
}

#[test]
fn test_get_ledger_state_root_differs_from_zswap_state_root() {
	mock::new_test_ext().execute_with(|| {
		init_ledger_state(BlockContext::default());

		let ledger_root = mock::Midnight::get_ledger_state_root().unwrap();
		let zswap_root = mock::Midnight::get_zswap_state_root().unwrap();

		assert_ne!(ledger_root, zswap_root);
	});
}

/// The first ledger transaction of a historical block is verified against the *parent* block's
/// timestamp, not the block's own: DEPLOY_TX's intent no longer verifies a minute before its own
/// block, so registering the correction must turn an accepted block into a rejected one.
///
/// See <https://github.com/midnightntwrk/midnight-node/issues/1924>
#[test]
fn test_tblock_correction_verifies_first_tx_against_parent_timestamp() {
	let (tx, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
	let block_context: BlockContext = block_context.into();
	let parent_ts = block_context.tblock - 60;

	// Control: with no correction configured the tx is verified at the block's own timestamp.
	ext_with_correction(None).execute_with(|| {
		init_ledger_state(block_context.clone());
		begin_block(1, parent_ts, uncached_block_ts(&block_context));

		assert_ok!(send_mn_transaction(tx.clone()));
	});

	// With the correction configured the same tx is verified at `parent_ts` instead.
	ext_with_correction(Some(parent_base_correction(u64::MAX))).execute_with(|| {
		init_ledger_state(block_context.clone());
		begin_block(1, parent_ts, uncached_block_ts(&block_context));

		assert!(
			send_mn_transaction(tx.clone()).is_err(),
			"the first tx in the block must be verified at the parent block's timestamp"
		);
	});
}

/// `disable_after` is compared against the block's own timestamp, so a block at the cutoff is
/// verified without any correction — the same block that the test above has rejected.
#[test]
fn test_tblock_correction_not_applied_at_or_after_disable_after() {
	let (tx, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
	let block_context: BlockContext = block_context.into();
	let parent_ts = block_context.tblock - 60;
	let block_ts = uncached_block_ts(&block_context);

	ext_with_correction(Some(parent_base_correction(block_ts))).execute_with(|| {
		init_ledger_state(block_context.clone());
		begin_block(1, parent_ts, block_ts);

		assert_ok!(send_mn_transaction(tx.clone()));
	});
}

/// Mempool ingress must not be corrected: `validate_unsigned` already skews the block context it
/// passes to the ledger by `slot_duration * (1 + MaxSkippedSlots)`, so correcting there too would
/// double-count a slot and reject valid transactions.
///
/// Both halves run in the same externalities, against the same block, with the same correction
/// registered — only the entry point differs.
#[test]
fn test_tblock_correction_does_not_affect_mempool_validation() {
	let (tx, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
	let block_context: BlockContext = block_context.into();
	let parent_ts = block_context.tblock - 60;

	ext_with_correction(Some(parent_base_correction(u64::MAX))).execute_with(|| {
		init_ledger_state(block_context.clone());
		begin_block(1, parent_ts, uncached_block_ts(&block_context));

		let call = MidnightCall::send_mn_transaction { midnight_tx: tx.clone() };
		assert_ok!(<mock::Midnight as ValidateUnsigned>::validate_unsigned(
			TransactionSource::External,
			&call
		));

		assert!(
			send_mn_transaction(tx).is_err(),
			"the block path must still apply the correction the mempool path ignores"
		);
	});
}

// ---------------------------------------------------------------------------------------------
// v12 -> v13 activation (sub-01 phase 5)
//
// Two things are under test here, and they are separable. First, *routing*: the host API now
// decodes through the version-preserving envelope, so a pre-memo `transaction[v12]` reaches
// validation and application at all — before this phase the strict single-version reader
// rejected it as undecodable. Second, *activation*: a memo-capable `transaction[v13]` is
// accepted only in a candidate block at or after the configured height, and is rejected without
// touching state below it, while the pre-memo encoding is accepted at every height, forever.
//
// The fixture is `DEPLOY_TX`, which carries no memo, so it has a valid encoding in both eras and
// the two can be compared directly. `encode_as_prior_version` produces the v12 one.
// ---------------------------------------------------------------------------------------------

/// `DEPLOY_TX` in both wire encodings, with the block context it was built for.
fn deploy_tx_both_encodings() -> (Vec<u8>, Vec<u8>, BlockContext) {
	let (v13, block_context) =
		midnight_node_ledger_helpers::ledger_9::extract_tx_with_context(DEPLOY_TX);
	let v12 = midnight_node_ledger::ledger_9::tx_envelope::encode_as_prior_version(&v13)
		.expect("the deploy fixture carries no memo, so it has a v12 encoding");
	(v12, v13, block_context.into())
}

fn set_activation_height(height: u64) {
	crate::pallet::MemoActivationHeight::<Test>::put(height);
}

/// Routing plus equivalence in one assertion: the pre-memo encoding is applied through the host
/// path, and leaves the ledger in exactly the state the memo-capable encoding of the same
/// transaction leaves it in. Same events, same state root — the encoding is not observable in
/// consensus state, which is the property that lets the two coexist indefinitely (spec FR-011).
#[test]
fn v12_and_v13_encodings_apply_to_the_same_state() {
	let (v12, v13, block_context) = deploy_tx_both_encodings();

	let roots_and_events = |tx: Vec<u8>| {
		mock::new_test_ext().execute_with(|| {
			init_ledger_state(block_context.clone());
			assert_ok!(send_mn_transaction(tx));
			let events = mock::midnight_events();
			assert_matches!(events[0], Event::ContractDeploy(_));
			assert_matches!(events[1], Event::TxApplied(_));
			(
				mock::Midnight::get_ledger_state_root().expect("state root"),
				mock::Midnight::get_zswap_state_root().expect("zswap root"),
				events.len(),
			)
		})
	};

	assert_eq!(
		roots_and_events(v12),
		roots_and_events(v13),
		"the two encodings of one transaction must leave identical state",
	);
}

/// Below activation, a memo-capable transaction fails admission with the structured
/// not-yet-active error and mutates nothing (spec FR-009). `u64::MAX` is the "never, on this
/// configuration" end of the range; the boundary cases are the two tests after this one.
#[test]
fn v13_is_rejected_and_mutates_nothing_before_activation() {
	let (_v12, v13, block_context) = deploy_tx_both_encodings();

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context);
		set_activation_height(u64::MAX);

		let ledger_root_before = mock::Midnight::get_ledger_state_root().expect("state root");
		let zswap_root_before = mock::Midnight::get_zswap_state_root().expect("zswap root");

		let error: sp_runtime::DispatchError = Error::<Test>::TransactionVersionNotActive.into();
		assert_err!(send_mn_transaction(v13), error);

		assert!(mock::midnight_events().is_empty(), "a rejected transaction emits no events");
		assert_eq!(
			mock::Midnight::get_ledger_state_root().expect("state root"),
			ledger_root_before,
			"rejection before activation must not mutate the ledger state",
		);
		assert_eq!(
			mock::Midnight::get_zswap_state_root().expect("zswap root"),
			zswap_root_before,
			"rejection before activation must not mutate the zswap state",
		);
	});
}

/// The pre-memo encoding is unaffected by the activation height: it is accepted at any height,
/// including one at which the memo-capable encoding is still rejected (spec FR-011).
#[test]
fn v12_is_accepted_before_activation() {
	let (v12, _v13, block_context) = deploy_tx_both_encodings();

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context);
		set_activation_height(u64::MAX);

		assert_ok!(send_mn_transaction(v12));
		assert_matches!(mock::midnight_events()[1], Event::TxApplied(_));
	});
}

/// The exact boundary. `init_ledger_state` puts the candidate block at height 1, so activation at
/// 1 accepts and activation at 2 — the very next block — rejects. Together with the test above,
/// this pins the comparison as `candidate_height >= activation_height`, not `>`.
#[test]
fn v13_activates_at_exactly_the_configured_height() {
	let (_v12, v13, block_context) = deploy_tx_both_encodings();
	const CANDIDATE_HEIGHT: u64 = 1;

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context.clone());
		assert_eq!(mock::System::block_number(), CANDIDATE_HEIGHT);
		set_activation_height(CANDIDATE_HEIGHT);

		assert_ok!(send_mn_transaction(v13.clone()));
	});

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context);
		set_activation_height(CANDIDATE_HEIGHT + 1);

		let error: sp_runtime::DispatchError = Error::<Test>::TransactionVersionNotActive.into();
		assert_err!(send_mn_transaction(v13), error);
	});
}

/// The default is activation at genesis, which is what every network this repository ships a
/// chain spec for uses — so nothing about today's behaviour changes, and the rest of this file
/// (which never sets a height) keeps passing for the right reason rather than by accident.
#[test]
fn the_default_activation_height_is_genesis() {
	mock::new_test_ext().execute_with(|| {
		assert_eq!(crate::pallet::MemoActivationHeight::<Test>::get(), 0);
		assert!(
			mock::Midnight::consensus_context().memo_active(),
			"with activation at 0 the memo-capable encoding is active from the first block",
		);
	});
}

/// Pool admission applies the same rule against the same candidate context, and reports it as the
/// structured not-yet-active code rather than a generic failure — the submitter is meant to be
/// able to tell "resubmit after activation" from "this transaction is broken" (spec FR-012).
/// There is no staging: the transaction is simply not admitted.
#[test]
fn the_pool_rejects_v13_before_activation_and_admits_v12() {
	let (v12, v13, block_context) = deploy_tx_both_encodings();

	mock::new_test_ext().execute_with(|| {
		init_ledger_state(block_context);
		set_activation_height(u64::MAX);

		let call = MidnightCall::send_mn_transaction { midnight_tx: v13 };
		assert_err!(
			<mock::Midnight as ValidateUnsigned>::validate_unsigned(
				TransactionSource::External,
				&call
			),
			TransactionValidityError::Invalid(InvalidTransaction::Custom(
				LedgerApiError::TransactionVersionNotActive.into()
			))
		);

		// `pre_dispatch` is the block-production side of the same rule: a transaction the pool
		// refuses must not be includable either (spec FR-013).
		assert_err!(
			<mock::Midnight as ValidateUnsigned>::pre_dispatch(&call),
			TransactionValidityError::Invalid(InvalidTransaction::Custom(
				LedgerApiError::TransactionVersionNotActive.into()
			))
		);

		let v12_call = MidnightCall::send_mn_transaction { midnight_tx: v12 };
		assert_ok!(<mock::Midnight as ValidateUnsigned>::validate_unsigned(
			TransactionSource::External,
			&v12_call
		));
	});
}

#[cfg(feature = "experimental")]
#[ignore = "TODO UNSHIELDED - fix when Claim Mint is properly handled for Unshielded"]
#[test]
fn test_send_claim_mint() {
	/*
	test commented out because it references block_rewards which no longer exist
		use crate::mock::BeneficiaryId;
		use frame_support::{
			pallet_prelude::ProvideInherent,
			traits::{OnFinalize, UnfilteredDispatchable},
		};
		use midnight_node_res::undeployed::transactions::CLAIM_MINT_TX;
		use sp_inherents::InherentData;

		mock::new_test_ext().execute_with(|| {
			init_ledger_state(BlockContext::default());

			let mut inherent_data = InherentData::new();

			let block_beneficiary_provider = sp_block_rewards::BlockBeneficiaryInherentProvider::<
				BeneficiaryId,
			>::from_env("SIDECHAIN_BLOCK_BENEFICIARY")
			.expect("SIDECHAIN_BLOCK_BENEFICIARY env variable not provided");

			inherent_data
				.put_data(
					sp_block_rewards::INHERENT_IDENTIFIER,
					&block_beneficiary_provider.beneficiary_id,
				)
				.unwrap();

			let call = <mock::BlockRewards as ProvideInherent>::create_inherent(&inherent_data)
				.expect("Creating test inherent should not fail");

			call.dispatch_bypass_filter(RuntimeOrigin::none())
				.expect("dispatching test call should work");

			mock::Midnight::on_finalize(mock::System::block_number());
			let events = mock::midnight_events();

			assert_matches!(events[0], Event::PayoutMinted(_));

			assert_ok!(mock::Midnight::send_mn_transaction(
				RuntimeOrigin::none(),
				hex::encode(CLAIM_MINT_TX).into_bytes()
			));
		});
	*/
}
// grcov-excl-stop
