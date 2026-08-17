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

use alloc::vec::Vec;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::prelude::string::String;
use scale_info_derive::TypeInfo;

pub const PERSISTENT_HASH_BYTES: usize = 32;
pub type Hash = [u8; PERSISTENT_HASH_BYTES];
pub struct WrappedHash(pub Hash);

const TWOX128_HASH_BYTES: usize = 16;
pub type Hash128 = [u8; TWOX128_HASH_BYTES];

impl From<Hash128> for WrappedHash {
	fn from(value: Hash128) -> Self {
		let mut extended = [0u8; PERSISTENT_HASH_BYTES]; // Create a new [u8; 32] array filled with zeros
		extended[..TWOX128_HASH_BYTES].copy_from_slice(&value); // Copy the original [u8; 16] into the first part
		WrappedHash(extended) // Return the extended array
	}
}

/// Consensus-authoritative data — beyond the block's own clock, which [`BlockContext`] already
/// carries — that decides which transaction *wire versions* a candidate block may contain.
///
/// [`BlockContext`]: crate::latest::BlockContext
///
/// Both fields come from the candidate block's own position and its parent state, never from
/// local wall-clock time, startup order, or a node-local setting, so a reorganization
/// re-evaluates activation by construction: the replacement branch supplies its own height and
/// reads its own chain configuration (spec FR-007/FR-010).
///
/// This is a separate struct rather than two more `BlockContext` fields because `BlockContext`
/// mirrors the ledger's own type and is shared by every ledger generation, while this data is
/// meaningful only from ledger 9 on. Keeping it apart also keeps the host-interface versioning
/// honest: the ledger-9 bridge grew a *second* version of each transaction entry point to carry
/// it, and the original version — which historical runtimes still call — is unchanged.
#[derive(Encode, Decode, DecodeWithMemTracking, TypeInfo, Clone, Debug, Eq, PartialEq, Default)]
pub struct ConsensusContext {
	/// Height of the candidate block whose validity is being decided. On the pool path this is
	/// the height of the block the submission would first be eligible for, i.e. parent + 1.
	pub block_height: u64,
	/// First block height at which memo-capable transactions are accepted. `0` — the default,
	/// and the deployed value for dev/undeployed/test networks — means active from genesis, so
	/// nothing changes for a chain that never had pre-memo history.
	pub memo_activation_height: u64,
}

impl ConsensusContext {
	/// A context in which memos are never active, used for the *original* version of each
	/// ledger-9 transaction host function. Those are the entry points historical runtimes call,
	/// and a runtime that predates this change is by definition a runtime under which the memo
	/// upgrade has not activated: it must keep accepting pre-memo transactions and keep
	/// rejecting memo-capable ones, exactly as it did before.
	pub const MEMO_NEVER_ACTIVE: Self =
		ConsensusContext { block_height: 0, memo_activation_height: u64::MAX };

	/// Whether the memo-capable wire version is accepted in this candidate block.
	pub fn memo_active(&self) -> bool {
		self.block_height >= self.memo_activation_height
	}
}

/// Which transaction wire encoding a submission arrived in, as far as the memo activation
/// boundary is concerned.
///
/// The distinction is about the *encoding*, not about whether a memo is present: a memo-less
/// transaction has a valid encoding in both eras, and which one it was sent in is what the
/// activation rule is written against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxWireEra {
	/// The pre-memo encoding — `transaction[v12]` under ledger 9, and the only encoding earlier
	/// ledger generations have. Accepted at any height, indefinitely (spec FR-011).
	PreMemo,
	/// The memo-capable encoding, `transaction[v13]`. Gated on the activation height.
	MemoCapable,
}

#[derive(Encode, Decode, DecodeWithMemTracking)]
pub struct TransactionApplied {
	pub tx_hash: Hash,
	pub all_applied: bool,
	pub new_state: Vec<u8>,
	pub call_addresses: Vec<Vec<u8>>,
	pub deploy_addresses: Vec<Vec<u8>>,
	pub maintain_addresses: Vec<Vec<u8>>,
	pub claim_rewards: Vec<u128>,
}

#[derive(Encode, Decode, DecodeWithMemTracking)]
pub struct TransactionAppliedStateRoot {
	pub state_root: Vec<u8>,
	pub tx_hash: Hash,
	pub all_applied: bool,
	pub call_addresses: Vec<Vec<u8>>,
	pub deploy_addresses: Vec<Vec<u8>>,
	pub maintain_addresses: Vec<Vec<u8>>,
	pub claim_rewards: Vec<u128>,
	pub unshielded_utxos_created: Vec<UtxoInfo>,
	pub unshielded_utxos_spent: Vec<UtxoInfo>,
}

#[derive(Encode, Decode, DecodeWithMemTracking)]
pub struct SystemTransactionAppliedStateRoot {
	pub state_root: Vec<u8>,
	pub tx_hash: Hash,
	pub tx_type: String,
}

#[derive(Encode, Decode, DecodeWithMemTracking, TypeInfo, Clone, Eq, PartialEq, Debug)]
pub enum Op {
	Call { address: Vec<u8>, entry_point: Vec<u8> },
	Deploy { address: Vec<u8> },
	Maintain { address: Vec<u8> },
	ClaimRewards { value: u128 },
	// New variants MUST be appended at the end: `Op` is SCALE-encoded across the
	// `get_decoded_transaction` runtime-API boundary, so the variant indices are wire-significant.
	// Appending keeps existing indices stable (an older decoder simply never sees the new index).
	ClaimBridgeTransfer { value: u128 },
}

#[derive(Encode, Decode, DecodeWithMemTracking, TypeInfo, Clone, Eq, PartialEq, Debug)]
pub struct Tx {
	pub hash: Hash,
	pub operations: Vec<Op>,
	pub identifiers: Vec<Vec<u8>>,
	pub has_fallible_coins: bool,
	pub has_guaranteed_coins: bool,
}

pub type StorageCost = u128;
pub type GasCost = u64;

#[derive(Encode, Decode, DecodeWithMemTracking, Default, Debug, Eq, PartialEq, Clone)]
pub struct GuaranteedCoinsDetails {
	inputs_num: u32,
	outputs_num: u32,
	transients_num: u32,
}

impl GuaranteedCoinsDetails {
	pub fn new(inputs_num: u32, outputs_num: u32, transients_num: u32) -> Self {
		Self { inputs_num, outputs_num, transients_num }
	}
}

#[derive(Encode, Decode, DecodeWithMemTracking, Default, Debug, Eq, PartialEq, Clone)]
pub struct FallibleCoinsDetails {
	inputs_num: u32,
	outputs_num: u32,
	transients_num: u32,
}

impl FallibleCoinsDetails {
	pub fn new(inputs_num: u32, outputs_num: u32, transients_num: u32) -> Self {
		Self { inputs_num, outputs_num, transients_num }
	}
}

#[derive(Encode, Decode, DecodeWithMemTracking, Default, Debug, Eq, PartialEq, Clone)]
pub struct MaintainUpdatesDetails {
	replace_authority_num: u32,
	verifier_key_remove_num: u32,
	verifier_key_insert_num: u32,
}

#[derive(Encode, Decode, DecodeWithMemTracking, Default, Debug, PartialEq, Eq, Clone)]
pub struct ContractCallsDetails {
	calls_gas_cost: GasCost,
	calls_num: u32,
	deploys_num: u32,
	mainatain_updates: MaintainUpdatesDetails,
}

impl ContractCallsDetails {
	#[inline]
	pub fn set_gas_cost(&mut self, gas: GasCost) {
		self.calls_gas_cost = gas;
	}

	#[inline]
	pub fn inc_calls(&mut self) {
		self.calls_num = self.calls_num.saturating_add(1);
	}

	#[inline]
	pub fn num_calls(&self) -> u32 {
		self.calls_num
	}

	#[inline]
	pub fn calls_gas_cost(&self) -> GasCost {
		self.calls_gas_cost
	}

	#[inline]
	pub fn inc_deploys(&mut self) {
		self.deploys_num = self.deploys_num.saturating_add(1);
	}

	#[inline]
	pub fn inc_replace_authority(&mut self) {
		self.mainatain_updates.replace_authority_num =
			self.mainatain_updates.replace_authority_num.saturating_add(1);
	}

	#[inline]
	pub fn inc_verifier_key_remove(&mut self) {
		self.mainatain_updates.verifier_key_remove_num =
			self.mainatain_updates.verifier_key_remove_num.saturating_add(1);
	}

	#[inline]
	pub fn inc_verifier_key_insert(&mut self) {
		self.mainatain_updates.verifier_key_insert_num =
			self.mainatain_updates.verifier_key_insert_num.saturating_add(1);
	}
}

#[derive(Encode, Decode, DecodeWithMemTracking, Debug, Eq, PartialEq, Clone)]
pub enum TransactionDetails {
	Standard {
		guaranteed_coins: GuaranteedCoinsDetails,
		fallible_coins: FallibleCoinsDetails,
		contract_calls: ContractCallsDetails,
	},
	ClaimRewards,
}

impl TransactionDetails {
	pub fn get_type(&self) -> &'static str {
		match self {
			Self::Standard { .. } => "standard",
			Self::ClaimRewards { .. } => "claim_rewards",
		}
	}
}

impl From<TransactionDetails> for GasCost {
	fn from(tx_details: TransactionDetails) -> Self {
		match tx_details {
			TransactionDetails::Standard { contract_calls, .. } => contract_calls.calls_gas_cost(),
			_ => 0,
		}
	}
}

#[derive(Encode, Decode, DecodeWithMemTracking, Clone, Eq, PartialEq)]
pub struct BenchmarkComponents {
	pub num_guaranteed_inputs: u32,
	pub num_guaranteed_outputs: u32,
	pub num_guaranteed_transients: u32,
	pub num_fallible_inputs: u32,
	pub num_fallible_outputs: u32,
	pub num_fallible_transients: u32,
	pub num_contracts_deploy: u32,
	pub num_contract_replace_auth: u32,
	pub num_contract_key_remove: u32,
	pub num_contract_key_insert: u32,
	pub num_contract_operations: u32,
}

impl TryFrom<&TransactionDetails> for BenchmarkComponents {
	type Error = &'static str;

	fn try_from(tx_details: &TransactionDetails) -> Result<Self, Self::Error> {
		match tx_details {
			TransactionDetails::Standard { guaranteed_coins, fallible_coins, contract_calls } => {
				Ok(BenchmarkComponents {
					num_guaranteed_inputs: guaranteed_coins.inputs_num,
					num_guaranteed_outputs: guaranteed_coins.outputs_num,
					num_guaranteed_transients: guaranteed_coins.transients_num,
					num_fallible_inputs: fallible_coins.inputs_num,
					num_fallible_outputs: fallible_coins.outputs_num,
					num_fallible_transients: fallible_coins.transients_num,
					num_contracts_deploy: contract_calls.deploys_num,
					num_contract_replace_auth: contract_calls
						.mainatain_updates
						.replace_authority_num,
					num_contract_key_remove: contract_calls
						.mainatain_updates
						.verifier_key_remove_num,
					num_contract_key_insert: contract_calls
						.mainatain_updates
						.verifier_key_insert_num,
					num_contract_operations: contract_calls.calls_num,
				})
			},
			_ => Err("Not possible to convert `BenchmarkComponents` from `TransactionDetails`"),
		}
	}
}

/// Type to help with custom Standard `Transaction<ProofMarker, D>` for benchmarks
#[derive(Encode, Decode, DecodeWithMemTracking, Clone)]
pub struct BenchmarkStandardTxBuilder {
	pub genesis_seed: Vec<u8>,
	pub wallet_seed: Vec<u8>,
	pub mint_amount: u128,
	pub fee_per_tx: u128,
	pub token: Vec<u8>,
	pub alt_token: Vec<u8>,
	pub benchmark_components: BenchmarkComponents,
}

/// Type to help with custom Standard `Transaction<ProofMarker, D>` for benchmarks
#[derive(Encode, Decode, DecodeWithMemTracking, Clone)]
pub struct BenchmarkClaimMintTxBuilder {
	pub wallet_seed: Vec<u8>,
	pub claim_amount: u128,
	pub token: Vec<u8>,
}

pub type SegmentId = u16;

#[derive(Encode, Decode, DecodeWithMemTracking, Debug, Clone, PartialEq, TypeInfo)]
pub struct UtxoInfo {
	pub address: Hash,
	pub token_type: Hash,
	pub intent_hash: Hash,
	pub value: u128,
	pub output_no: u32,
}
