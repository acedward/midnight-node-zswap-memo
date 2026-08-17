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

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
};

use super::ledger_helpers_local::{
	BuildInput, BuildIntent, BuildOutput, BuildUtxoOutput, BuildUtxoSpend, BuilderContext,
	CoinSelectionStrategy, DefaultDB, FromContext as _, InputInfo, IntentInfo, OfferInfo,
	OutputInfo, ProofProvider, Segment, ShieldedCoinSelectionError, ShieldedTokenType,
	StandardTrasactionInfo, TransactionWithContext, UnshieldedOfferInfo, UnshieldedTokenType,
	UtxoId, UtxoOutputInfo, UtxoSelectionError, UtxoSpendInfo, WalletSeed, validate_shielded_memo,
};
use super::output_spec::{
	ShieldedOutputSpec, UnshieldedOutputSpec, clone_shielded_spec, clone_unshielded_spec,
	legacy_to_output_args, resolve_outputs_from_triples,
};
use async_trait::async_trait;

use crate::{
	progress::Spin,
	serde_def::SourceTransactions,
	tx_generator::builder::{BuildTxs, SingleTxArgs},
};
use midnight_node_ledger_helpers::fork::raw_block_data::SerializedTxBatches;

pub(crate) const MAX_GUARANTEED_OUTPUTS: usize = 2;
const MAX_GUARANTEED_INPUTS_OUTPUTS: usize = 3;

#[derive(Debug, thiserror::Error)]
pub enum SingleTxError {
	#[error("failed to select unshielded UTXOs for transfer: {0}")]
	UtxoSelection(#[from] UtxoSelectionError),
	#[error("insufficient shielded coins for transfer: {0}")]
	ShieldedCoinSelection(#[from] ShieldedCoinSelectionError),
	#[error("proving failed: {0}")]
	ProvingFailed(String),
	#[error("transaction is empty: no valid destination addresses were resolved into outputs")]
	EmptyTransaction,
	#[error(
		"--memo requires ledger 9 or later, but this transaction targets ledger {version}; \
		 earlier ledgers have no field to carry a memo"
	)]
	MemoUnsupportedLedger { version: u32 },
	#[error("invalid --memo: {0}")]
	InvalidMemo(#[source] midnight_node_ledger_helpers::ShieldedSpendError),
	#[error(
		"--memo applies to the shielded spend, but this transaction has no shielded output; \
		 add a shielded --output or drop --memo"
	)]
	MemoWithoutShieldedSpend,
}

/// Rejects an impossible memo request before progress UI, context access, coin selection, or
/// proving can have side effects. This common module is compiled once for each supported ledger
/// generation, so `validate_shielded_memo` is the generation-specific source of truth.
fn preflight_memo_request(
	memo: Option<&[u8]>,
	has_shielded_output: bool,
) -> Result<(), SingleTxError> {
	let Some(memo) = memo else {
		return Ok(());
	};

	match validate_shielded_memo(Some(memo)) {
		Ok(()) => {},
		Err(midnight_node_ledger_helpers::ShieldedSpendError::MemoUnsupportedLedger {
			version,
		}) => return Err(SingleTxError::MemoUnsupportedLedger { version }),
		Err(error) => return Err(SingleTxError::InvalidMemo(error)),
	}

	if !has_shielded_output {
		return Err(SingleTxError::MemoWithoutShieldedSpend);
	}

	Ok(())
}

pub struct SingleTxBuilder<C: BuilderContext<DefaultDB>> {
	context: Arc<C>,
	prover: Arc<dyn ProofProvider<DefaultDB>>,
	shielded_outputs: Vec<ShieldedOutputSpec<DefaultDB>>,
	unshielded_outputs: Vec<UnshieldedOutputSpec>,
	source_seed: WalletSeed,
	funding_seed: Option<WalletSeed>,
	input_utxos: Vec<UtxoId>,
	rng_seed: Option<[u8; 32]>,
	coin_selection: CoinSelectionStrategy,
	memo: Option<Vec<u8>>,
}

impl<C: BuilderContext<DefaultDB>> SingleTxBuilder<C> {
	pub fn new(
		args: SingleTxArgs,
		context: Arc<C>,
		prover: Arc<dyn ProofProvider<DefaultDB>>,
	) -> Result<Self, SingleTxError> {
		use super::type_convert::*;

		// Run memo validation before any legacy CLI-shape normalization. In particular, a memo with
		// no destination is a typed bad request; it must not reach the historical
		// `no destinations provided` panic below. Other malformed legacy shapes retain their existing
		// behavior once this memo-specific boundary has succeeded.
		let has_destination = !args.outputs.is_empty() || !args.destination_address.is_empty();
		preflight_memo_request(
			args.memo.as_ref().map(crate::cli_parsers::MemoArg::as_bytes),
			has_destination,
		)?;

		// CLI shape selection. Two shapes are accepted; mixing is a usage error.
		//   (A) --output triples (address+amount+token bundled per flag)
		//   (B) --destination-address + parallel --*-amount / --*-token-type lists
		let any_legacy_amount = !args.shielded_amount.is_empty()
			|| !args.unshielded_amount.is_empty()
			|| !args.shielded_token_type.is_empty()
			|| !args.unshielded_token_type.is_empty()
			|| !args.destination_address.is_empty();

		// Normalise the two CLI shapes into a single `Vec<OutputArg>` so the
		// downstream resolution and HRP-classification logic has one entry point.
		let output_args: Vec<crate::cli_parsers::OutputArg> = if !args.outputs.is_empty() {
			if any_legacy_amount {
				log::error!(
					"--output cannot be combined with --destination-address / --shielded-amount / --shielded-token-type / --unshielded-amount / --unshielded-token-type; pick one CLI shape"
				);
				panic!("mixed CLI shapes");
			}
			args.outputs.clone()
		} else {
			if args.destination_address.is_empty() {
				log::error!(
					"single-tx requires at least one destination: pass --output addr=...,amount=...[,token=...] or --destination-address <ADDRESS>"
				);
				panic!("no destinations provided");
			}
			legacy_to_output_args(&args)
		};
		let (shielded_outputs, unshielded_outputs) = resolve_outputs_from_triples(&output_args);
		preflight_memo_request(
			args.memo.as_ref().map(crate::cli_parsers::MemoArg::as_bytes),
			!shielded_outputs.is_empty(),
		)?;

		// The builder stores only the seed value; the unshielded signature scheme is applied when
		// the context wallet is built (see `Builder::relevant_wallet_schemes`), so the scheme half
		// of each resolved pair is dropped here.
		let (source_seed, _) = args.source_seed.resolve();
		let funding_seed = args.funding_seed.map(|s| s.resolve().0);

		Ok(Self {
			context,
			prover,
			shielded_outputs,
			unshielded_outputs,
			source_seed: convert_wallet_seed(source_seed),
			funding_seed: funding_seed.map(convert_wallet_seed),
			input_utxos: {
				let mut seen: HashSet<([u8; 32], u32)> = HashSet::new();
				args.input_utxos
					.iter()
					.filter(|id| seen.insert((id.intent_hash.0.0, id.output_number)))
					.map(convert_utxo_id)
					.collect()
			},
			rng_seed: args.rng_seed,
			coin_selection: args.coin_selection,
			memo: args.memo.clone().map(crate::cli_parsers::MemoArg::into_bytes),
		})
	}

	pub fn build() {}
}

#[async_trait]
impl<C: BuilderContext<DefaultDB>> BuildTxs for SingleTxBuilder<C> {
	type Error = SingleTxError;

	async fn build_txs_from(
		&self,
		_received_tx: SourceTransactions,
	) -> Result<SerializedTxBatches, Self::Error> {
		preflight_memo_request(self.memo.as_deref(), !self.shielded_outputs.is_empty())?;

		let spin = Spin::new("generating single tx...");

		let context = self.context.clone();
		let funding_seed = self.funding_seed.clone().unwrap_or(self.source_seed.clone());

		// - Transaction info
		let mut tx_info = StandardTrasactionInfo::new_from_context(
			context.clone(),
			self.prover.clone(),
			self.rng_seed,
		);

		if !self.shielded_outputs.is_empty() {
			let offer = build_shielded_offer(
				context.clone(),
				self.source_seed.clone(),
				self.shielded_outputs.iter().map(clone_shielded_spec).collect(),
				self.coin_selection,
				self.memo.clone(),
			)?;
			if offer.outputs.len() > MAX_GUARANTEED_OUTPUTS {
				tx_info.set_fallible_offers(HashMap::from([(1, offer)]));
			} else {
				tx_info.set_guaranteed_offer(offer);
			}
		}

		if !self.unshielded_outputs.is_empty() {
			let intents = build_unshielded_intents(
				context.clone(),
				self.source_seed.clone(),
				self.unshielded_outputs.iter().map(clone_unshielded_spec).collect(),
				&self.input_utxos,
				self.coin_selection,
			)
			.await?;
			tx_info.set_intents(intents);
		}

		tx_info.set_funding_seeds(vec![funding_seed]);
		tx_info.use_mock_proofs_for_fees(true);

		if tx_info.is_empty() {
			log::error!(
				"transaction is empty! No valid destination_addresses were resolved into outputs"
			);
			return Err(SingleTxError::EmptyTransaction);
		}

		let tx = tx_info
			.prove()
			.await
			.map_err(|e| SingleTxError::ProvingFailed(format!("{e}")))?;

		let tx_with_context = TransactionWithContext::new(tx, None);

		spin.finish("generated tx.");

		Ok(super::tx_serialization::build_single(tx_with_context))
	}
}

/// Attaches a requested memo to exactly the first selected shielded input across all token groups.
/// Change outputs are built separately and cannot become a carrier; keeping the cross-group state
/// explicit prevents a later group from receiving a second copy.
fn attach_memo_to_first_selected<O>(
	selection: &mut (Vec<InputInfo<O>>, u128),
	memo: Option<&[u8]>,
	memo_attached: &mut bool,
) {
	if *memo_attached {
		return;
	}
	let (Some(memo), Some(first_input)) = (memo, selection.0.first_mut()) else {
		return;
	};

	first_input.memo = Some(memo.to_vec());
	*memo_attached = true;
}

/// Build a shielded offer that may contain outputs of multiple distinct token
/// types. Inputs are selected separately per token type; one change output per
/// token type is appended when needed.
pub(crate) fn build_shielded_offer<C: BuilderContext<DefaultDB>>(
	context: Arc<C>,
	funding_seed: WalletSeed,
	outputs: Vec<ShieldedOutputSpec<DefaultDB>>,
	coin_selection: CoinSelectionStrategy,
	memo: Option<Vec<u8>>,
) -> Result<OfferInfo<DefaultDB, C>, ShieldedCoinSelectionError> {
	// Sum amounts per token type, in the order each token type first appears so
	// behaviour is deterministic for callers.
	let mut totals: Vec<(ShieldedTokenType, u128)> = Vec::new();
	for spec in &outputs {
		match totals.iter_mut().find(|(tt, _)| *tt == spec.token_type) {
			Some((_, sum)) => {
				*sum = sum
					.checked_add(spec.amount)
					.ok_or(ShieldedCoinSelectionError::ArithmeticOverflow)?;
			},
			None => totals.push((spec.token_type, spec.amount)),
		}
	}

	let mut inputs_info: Vec<Box<dyn BuildInput<DefaultDB, C>>> = Vec::new();
	let mut outputs_info: Vec<Box<dyn BuildOutput<DefaultDB, C>>> = Vec::new();
	let mut memo_attached = false;

	// User outputs first, in the order they were given.
	for spec in outputs {
		let output: Box<dyn BuildOutput<DefaultDB, C>> = Box::new(OutputInfo {
			destination: spec.wallet,
			token_type: spec.token_type,
			value: spec.amount,
		});
		outputs_info.push(output);
	}

	// Per token type: select inputs and append a change refund if needed.
	let select_start = std::time::Instant::now();
	for (token_type, total_required) in totals {
		let mut selection = InputInfo::coins_to_cover_value(
			context.clone(),
			funding_seed.clone(),
			total_required,
			token_type,
			coin_selection,
		)?;
		attach_memo_to_first_selected(&mut selection, memo.as_deref(), &mut memo_attached);
		let (token_inputs, change) = selection;

		for input in token_inputs {
			let input: Box<dyn BuildInput<DefaultDB, C>> = Box::new(input);
			inputs_info.push(input);
		}

		if change > 0 {
			let refund: Box<dyn BuildOutput<DefaultDB, C>> = Box::new(OutputInfo {
				destination: funding_seed.clone(),
				token_type,
				value: change,
			});
			outputs_info.push(refund);
		}
	}
	log::debug!("[perf] select_shielded_offer took {:?}", select_start.elapsed());

	// Fail loudly rather than hand back an offer quietly missing the memo that was asked for.
	if memo.is_some() && !memo_attached {
		return Err(ShieldedCoinSelectionError::MemoNotAttached);
	}

	Ok(OfferInfo { inputs: inputs_info, outputs: outputs_info, transients: vec![] })
}

/// Build the unshielded intents that may contain outputs of multiple distinct
/// token types. UTXOs are selected separately per token type; one change output
/// per token type is appended when needed.
///
/// `input_utxos`, when non-empty, pins the inputs used for the spend. This is
/// only supported when exactly one unshielded token type is used across all
/// outputs (the pinned UTXOs must all share that token type).
pub(crate) async fn build_unshielded_intents<C: BuilderContext<DefaultDB>>(
	context: Arc<C>,
	source_seed: WalletSeed,
	outputs: Vec<UnshieldedOutputSpec>,
	input_utxos: &[UtxoId],
	coin_selection: CoinSelectionStrategy,
) -> Result<HashMap<u16, Box<dyn BuildIntent<DefaultDB, C>>>, UtxoSelectionError> {
	// Sum amounts per token type, preserving first-seen order.
	let mut totals: Vec<(UnshieldedTokenType, u128)> = Vec::new();
	for spec in &outputs {
		match totals.iter_mut().find(|(tt, _)| *tt == spec.token_type) {
			Some((_, sum)) => {
				*sum =
					sum.checked_add(spec.amount).ok_or(UtxoSelectionError::ArithmeticOverflow)?;
			},
			None => totals.push((spec.token_type, spec.amount)),
		}
	}

	if !input_utxos.is_empty() && totals.len() > 1 {
		panic!(
			"--input-utxo is only supported when a single unshielded token type is used; got {} distinct types",
			totals.len()
		);
	}

	let mut inputs_info: Vec<Box<dyn BuildUtxoSpend<DefaultDB, C>>> = Vec::new();
	let mut outputs_info: Vec<Box<dyn BuildUtxoOutput<DefaultDB, C>>> = Vec::new();

	// User outputs first, in the order they were given.
	for spec in outputs {
		let output: Box<dyn BuildUtxoOutput<DefaultDB, C>> = Box::new(UtxoOutputInfo {
			value: spec.amount,
			owner: spec.wallet,
			token_type: spec.token_type,
		});
		outputs_info.push(output);
	}

	// Per token type: select utxos (or use pinned utxos for the single-token case)
	// and append a change refund if needed.
	let select_start = std::time::Instant::now();
	for (token_type, total_required) in totals {
		let (token_inputs, remaining) = if input_utxos.is_empty() {
			UtxoSpendInfo::utxos_to_cover_value(
				context.clone(),
				source_seed.clone(),
				total_required,
				token_type,
				coin_selection,
			)
			.await?
		} else {
			UtxoSpendInfo::utxos_by_ids(
				context.clone(),
				source_seed.clone(),
				total_required,
				token_type,
				input_utxos,
			)
			.await?
		};

		for input in token_inputs {
			let input: Box<dyn BuildUtxoSpend<DefaultDB, C>> = Box::new(input);
			inputs_info.push(input);
		}

		if remaining > 0 {
			let refund: Box<dyn BuildUtxoOutput<DefaultDB, C>> = Box::new(UtxoOutputInfo {
				value: remaining,
				owner: source_seed.clone(),
				token_type,
			});
			outputs_info.push(refund);
		}
	}
	log::debug!("[perf] select_unshielded_intents took {:?}", select_start.elapsed());

	let inputs_outputs_len = inputs_info.len() + outputs_info.len();
	let unshielded_offer = UnshieldedOfferInfo { inputs: inputs_info, outputs: outputs_info };

	let intent_info = if inputs_outputs_len > MAX_GUARANTEED_INPUTS_OUTPUTS {
		IntentInfo {
			guaranteed_unshielded_offer: None,
			fallible_unshielded_offer: Some(unshielded_offer),
			actions: vec![],
		}
	} else {
		IntentInfo {
			guaranteed_unshielded_offer: Some(unshielded_offer),
			fallible_unshielded_offer: None,
			actions: vec![],
		}
	};
	let boxed_intent: Box<dyn BuildIntent<DefaultDB, C>> = Box::new(intent_info);

	let mut intents = HashMap::new();
	intents.insert(Segment::Fallible.into(), boxed_intent);

	Ok(intents)
}

#[cfg(test)]
mod tests {
	use super::super::ledger_helpers_local::{
		HashOutput, LEDGER_VERSION, LedgerContext, LocalProofServer, ShieldedWallet,
		UnshieldedWallet,
	};
	use super::*;

	fn test_seed() -> WalletSeed {
		WalletSeed::Short([0u8; 16])
	}

	fn test_seed_2() -> WalletSeed {
		WalletSeed::Short([1u8; 16])
	}

	fn test_context() -> Arc<LedgerContext<DefaultDB>> {
		Arc::new(LedgerContext::new("test"))
	}

	fn selected_input(value: u128, token_marker: u8) -> InputInfo<WalletSeed> {
		InputInfo {
			origin: test_seed(),
			token_type: ShieldedTokenType(HashOutput([token_marker; 32])),
			value,
			nullifier: None,
			memo: None,
		}
	}

	fn memo_request_without_a_destination() -> SingleTxArgs {
		SingleTxArgs {
			outputs: vec![],
			shielded_amount: vec![],
			shielded_token_type: vec![],
			unshielded_amount: vec![],
			unshielded_token_type: vec![],
			source_seed: crate::cli_parsers::scheme_seed_decode(
				"0000000000000000000000000000000000000000000000000000000000000001",
			)
			.expect("test source seed is valid"),
			funding_seed: None,
			destination_address: vec![],
			input_utxos: vec![],
			rng_seed: None,
			coin_selection: CoinSelectionStrategy::LargestFirst,
			memo: Some(
				crate::cli_parsers::MemoArg::new(vec![0x42]).expect("one-byte memo is valid"),
			),
		}
	}

	#[test]
	fn memo_preflight_uses_the_compiled_ledger_generation() {
		let memo = [0x42];
		let result = preflight_memo_request(Some(&memo), true);

		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				result,
				Err(SingleTxError::MemoUnsupportedLedger { version })
					if version == LEDGER_VERSION
			)),
			9 => assert!(result.is_ok(), "ledger 9 must accept a valid memo: {result:?}"),
			version => panic!("test does not define memo support for ledger {version}"),
		}

		assert!(
			preflight_memo_request(None, false).is_ok(),
			"omitting --memo must leave unshielded-only transactions valid"
		);
	}

	#[test]
	fn memo_preflight_rejects_an_impossible_carrier_before_building() {
		let result = preflight_memo_request(Some(&[0x42]), false);
		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				result,
				Err(SingleTxError::MemoUnsupportedLedger { version })
					if version == LEDGER_VERSION
			)),
			9 => assert!(matches!(result, Err(SingleTxError::MemoWithoutShieldedSpend))),
			version => panic!("test does not define memo support for ledger {version}"),
		}
	}

	#[test]
	fn constructing_a_memo_request_without_a_destination_returns_a_typed_error() {
		let prover: Arc<dyn ProofProvider<DefaultDB>> = Arc::new(LocalProofServer::new());
		let result =
			SingleTxBuilder::new(memo_request_without_a_destination(), test_context(), prover);
		let error = match result {
			Ok(_) => panic!("a memo request without a destination must be rejected"),
			Err(error) => error,
		};

		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				error,
				SingleTxError::MemoUnsupportedLedger { version } if version == LEDGER_VERSION
			)),
			9 => assert!(matches!(error, SingleTxError::MemoWithoutShieldedSpend)),
			version => panic!("test does not define memo support for ledger {version}"),
		}
	}

	#[test]
	fn memo_preflight_rejects_invalid_ledger_9_bytes() {
		let result = preflight_memo_request(Some(&[]), true);
		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				result,
				Err(SingleTxError::MemoUnsupportedLedger { version })
					if version == LEDGER_VERSION
			)),
			9 => assert!(matches!(result, Err(SingleTxError::InvalidMemo(_)))),
			version => panic!("test does not define memo support for ledger {version}"),
		}
	}

	#[test]
	fn memo_is_attached_once_across_input_groups_with_change() {
		let memo = [0xde, 0xad, 0xbe, 0xef];
		let mut memo_attached = false;
		let mut first_token_group = (vec![selected_input(60, 1), selected_input(40, 1)], 0);
		let mut second_token_group = (vec![selected_input(150, 2)], 50);

		attach_memo_to_first_selected(&mut first_token_group, Some(&memo), &mut memo_attached);
		attach_memo_to_first_selected(&mut second_token_group, Some(&memo), &mut memo_attached);

		let carriers: Vec<&[u8]> = first_token_group
			.0
			.iter()
			.chain(&second_token_group.0)
			.filter_map(|input| input.memo.as_deref())
			.collect();
		assert_eq!(carriers, vec![memo.as_slice()]);
		assert_eq!(first_token_group.1, 0);
		assert_eq!(second_token_group.1, 50, "change accounting must be left untouched");
	}

	#[test]
	fn memo_with_an_empty_wallet_returns_no_selected_input_error() {
		let context =
			Arc::new(LedgerContext::<DefaultDB>::new_from_wallet_seeds("test", &[test_seed()]));
		let token_type = ShieldedTokenType(HashOutput([0u8; 32]));
		let outputs = vec![ShieldedOutputSpec {
			wallet: ShieldedWallet::default(test_seed_2()),
			amount: 1,
			token_type,
		}];

		let result = build_shielded_offer(
			context,
			test_seed(),
			outputs,
			CoinSelectionStrategy::default(),
			Some(vec![0x42]),
		);

		assert!(matches!(
			result,
			Err(ShieldedCoinSelectionError::InsufficientBalance {
				required: 1,
				token_type: actual_token,
				..
			}) if actual_token == token_type
		));
	}

	#[test]
	fn memo_without_any_selected_group_returns_memo_not_attached() {
		let result = build_shielded_offer(
			test_context(),
			test_seed(),
			Vec::new(),
			CoinSelectionStrategy::default(),
			Some(vec![0x42]),
		);

		assert!(matches!(result, Err(ShieldedCoinSelectionError::MemoNotAttached)));
	}

	#[test]
	fn build_shielded_offer_mul_overflow_returns_arithmetic_error() {
		let context = test_context();
		let wallet1 = ShieldedWallet::default(test_seed());
		let wallet2 = ShieldedWallet::default(test_seed_2());
		let token_type = ShieldedTokenType(HashOutput([0u8; 32]));

		let outputs = vec![
			ShieldedOutputSpec { wallet: wallet1, amount: u128::MAX, token_type },
			ShieldedOutputSpec { wallet: wallet2, amount: u128::MAX, token_type },
		];

		let result = build_shielded_offer(
			context,
			test_seed(),
			outputs,
			CoinSelectionStrategy::default(),
			None,
		);

		assert!(matches!(result, Err(ShieldedCoinSelectionError::ArithmeticOverflow)));
	}

	#[tokio::test]
	async fn build_unshielded_intents_mul_overflow_returns_arithmetic_error() {
		let context = test_context();
		let wallet1 = UnshieldedWallet::default(test_seed());
		let wallet2 = UnshieldedWallet::default(test_seed_2());
		let token_type = UnshieldedTokenType(HashOutput([0u8; 32]));

		let outputs = vec![
			UnshieldedOutputSpec { wallet: wallet1, amount: u128::MAX, token_type },
			UnshieldedOutputSpec { wallet: wallet2, amount: u128::MAX, token_type },
		];

		let result = build_unshielded_intents(
			context,
			test_seed(),
			outputs,
			&[],
			CoinSelectionStrategy::default(),
		)
		.await;

		assert!(matches!(result, Err(UtxoSelectionError::ArithmeticOverflow)));
	}
}
