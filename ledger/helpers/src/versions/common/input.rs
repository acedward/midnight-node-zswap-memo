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

use super::{
	BuilderContext, DB, Input, Nullifier, ProofPreimage, QualifiedInfo, Segment, ShieldedTokenType,
	Sp, StdRng, TokenInfo, WalletSeed, WalletState, shielded_spend, validate_shielded_memo,
};
use crate::CoinSelectionStrategy;
use itertools::Itertools;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ShieldedCoinSelectionError {
	#[error(
		"insufficient shielded coins: need {required} of token {token_type:?} from seed {seed:?}"
	)]
	InsufficientBalance { required: u128, token_type: ShieldedTokenType, seed: WalletSeed },
	#[error("arithmetic overflow in shielded coin selection")]
	ArithmeticOverflow,
	#[error("a memo was supplied but no shielded input was selected to carry it")]
	MemoNotAttached,
	#[error("source shielded wallet is not registered in the builder context")]
	SourceWalletNotFound,
}

#[derive(Clone)]
pub struct InputInfo<O> {
	pub origin: O,
	pub token_type: ShieldedTokenType,
	pub value: u128,
	pub nullifier: Option<Nullifier>,
	/// An optional memo, bound into this spend's proof. Ledger 9 and later only.
	pub memo: Option<Vec<u8>>,
}

impl<O> TokenInfo for InputInfo<O> {
	fn token_type(&self) -> ShieldedTokenType {
		self.token_type
	}
	fn value(&self) -> u128 {
		self.value
	}
}

pub trait BuildInput<D: DB + Clone, C: BuilderContext<D>>: TokenInfo + Send + Sync {
	fn build(
		&mut self,
		rng: &mut StdRng,
		context: Arc<C>,
	) -> Result<Input<ProofPreimage, D>, crate::ShieldedSpendError>;
}

impl InputInfo<WalletSeed> {
	/// Find the least-valued coin satisfying this direct input request.
	///
	/// This is fallible because `InputInfo` is a public construction API: an empty wallet, an
	/// absent token, or a stale exact nullifier is ordinary input failure and must not abort the
	/// calling process.
	pub fn min_match_coin<D: DB + Clone>(
		&self,
		wallet: &WalletState<D>,
	) -> Result<Sp<QualifiedInfo, D>, crate::ShieldedSpendError> {
		let coins = wallet
			.coins
			.iter()
			.filter(|(nullifier, coin)| {
				if let Some(ref exact_nullifier) = self.nullifier {
					exact_nullifier == nullifier
				} else {
					coin.type_ == self.token_type && coin.value >= self.value
				}
			})
			.map(|(_nullifier, coin)| coin)
			.sorted_by_key(|coin| coin.value)
			.collect::<Vec<Sp<QualifiedInfo, D>>>();

		coins.first().cloned().ok_or_else(|| {
			if self.nullifier.is_some() {
				crate::ShieldedSpendError::ShieldedCoinNotFoundByNullifier
			} else {
				crate::ShieldedSpendError::NoMatchingShieldedCoin { minimum_value: self.value }
			}
		})
	}

	/// Returns a vector of InputInfo matching coins selected from the wallet to cover
	/// required_value of a token_type, plus the remaining change value.
	pub fn coins_to_cover_value<D: DB + Clone, C: BuilderContext<D>>(
		context: Arc<C>,
		seed: WalletSeed,
		required_value: u128,
		token_type: ShieldedTokenType,
		strategy: CoinSelectionStrategy,
	) -> Result<(Vec<InputInfo<WalletSeed>>, u128), ShieldedCoinSelectionError> {
		context
			.try_with_wallet_from_seed(seed.clone(), |wallet| {
				let matching_inputs: Vec<InputInfo<WalletSeed>> = wallet
					.shielded
					.state
					.coins
					.iter()
					.filter(|(_nullifier, coin)| coin.type_ == token_type)
					.map(|(nullifier, coin)| InputInfo {
						origin: seed.clone(),
						token_type,
						value: coin.value,
						nullifier: Some(nullifier),
						memo: None,
					})
					.collect();
				Self::select_inputs(matching_inputs, required_value, strategy).ok_or(
					ShieldedCoinSelectionError::InsufficientBalance {
						required: required_value,
						token_type,
						seed: seed.clone(),
					},
				)
			})
			.ok_or(ShieldedCoinSelectionError::SourceWalletNotFound)?
	}

	/// From given `inputs` select coins totaling at least `required`, ordered by `strategy`.
	/// Returns selected coins and change.
	fn select_inputs(
		mut inputs: Vec<InputInfo<WalletSeed>>,
		required: u128,
		strategy: CoinSelectionStrategy,
	) -> Option<(Vec<InputInfo<WalletSeed>>, u128)> {
		inputs.sort_by_key(|input| input.value);
		if matches!(strategy, CoinSelectionStrategy::LargestFirst) {
			inputs.reverse();
		}

		let mut total = 0u128;
		let mut selected = Vec::with_capacity(inputs.len());
		for input in inputs {
			total = total.checked_add(input.value)?;
			selected.push(input);
			if let Some(change) = total.checked_sub(required) {
				return Some((selected, change));
			}
		}
		None
	}
}

impl<D: DB + Clone, C: BuilderContext<D>> BuildInput<D, C> for InputInfo<WalletSeed> {
	fn build(
		&mut self,
		rng: &mut StdRng,
		context: Arc<C>,
	) -> Result<Input<ProofPreimage, D>, crate::ShieldedSpendError> {
		// Validate before looking up a coin. A malformed or unsupported memo is an ordinary bad
		// request and must return its typed error even if the wallet is empty.
		validate_shielded_memo(self.memo.as_deref())?;
		context
			.try_with_wallet_from_seed(self.origin.clone(), |wallet| {
				let coin: Sp<QualifiedInfo, D> = self.min_match_coin(&wallet.shielded.state)?;

				// Update the `InputInfo` value with the actual coin value that is going to be spent
				self.value = coin.value;

				let (updated_walet, input) = shielded_spend(
					&wallet.shielded.state,
					rng,
					wallet.shielded.secret_keys(),
					&coin,
					Segment::Guaranteed.into(),
					self.memo.clone(),
				)?;

				// Update wallet
				wallet.shielded.state = updated_walet;

				Ok(input)
			})
			.ok_or(crate::ShieldedSpendError::SourceWalletNotFound)?
	}
}

#[cfg(test)]
mod tests {
	use super::super::super::LEDGER_VERSION;
	use super::super::{DefaultDB, HashOutput, LedgerContext, SeedableRng};
	use super::*;

	fn test_seed() -> WalletSeed {
		WalletSeed::Short([0u8; 16])
	}

	fn test_token_type() -> ShieldedTokenType {
		ShieldedTokenType(HashOutput([0u8; 32]))
	}

	fn make_input(value: u128) -> InputInfo<WalletSeed> {
		InputInfo {
			origin: test_seed(),
			token_type: test_token_type(),
			value,
			nullifier: None,
			memo: None,
		}
	}

	#[test]
	fn invalid_or_unsupported_memo_is_typed_before_wallet_lookup() {
		// `LedgerContext::new` deliberately contains no wallet for `test_seed`. If `build` looks the
		// wallet up before validating the memo, this test panics instead of returning the typed error.
		for memo in [Vec::new(), vec![0u8; 513]] {
			let context = Arc::new(LedgerContext::<DefaultDB>::new("test"));
			let mut input = make_input(1);
			input.memo = Some(memo);
			let mut rng = StdRng::from_seed([0u8; 32]);
			let result: Result<Input<ProofPreimage, DefaultDB>, crate::ShieldedSpendError> =
				input.build(&mut rng, context);

			match LEDGER_VERSION {
				7 | 8 => assert!(matches!(
					result,
					Err(crate::ShieldedSpendError::MemoUnsupportedLedger { version })
						if version == LEDGER_VERSION
				)),
				9 => assert!(matches!(result, Err(crate::ShieldedSpendError::InvalidMemo(_)))),
				version => panic!("test does not define memo support for ledger {version}"),
			}
		}
	}

	#[test]
	fn valid_memo_with_an_empty_source_wallet_returns_a_typed_coin_error() {
		let context =
			Arc::new(LedgerContext::<DefaultDB>::new_from_wallet_seeds("test", &[test_seed()]));
		let mut input = make_input(1);
		input.memo = Some(vec![0x42]);
		let mut rng = StdRng::from_seed([0u8; 32]);
		let result: Result<Input<ProofPreimage, DefaultDB>, crate::ShieldedSpendError> =
			input.build(&mut rng, context);

		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				result,
				Err(crate::ShieldedSpendError::MemoUnsupportedLedger { version })
					if version == LEDGER_VERSION
			)),
			9 => assert!(matches!(
				result,
				Err(crate::ShieldedSpendError::NoMatchingShieldedCoin { minimum_value: 1 })
			)),
			version => panic!("test does not define memo support for ledger {version}"),
		}
	}

	#[test]
	fn valid_memo_with_an_unregistered_source_wallet_returns_a_typed_error() {
		let context = Arc::new(LedgerContext::<DefaultDB>::new("test"));
		let mut input = make_input(1);
		input.memo = Some(vec![0x42]);
		let mut rng = StdRng::from_seed([0u8; 32]);
		let result: Result<Input<ProofPreimage, DefaultDB>, crate::ShieldedSpendError> =
			input.build(&mut rng, context.clone());

		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				result,
				Err(crate::ShieldedSpendError::MemoUnsupportedLedger { version })
					if version == LEDGER_VERSION
			)),
			9 => assert!(matches!(result, Err(crate::ShieldedSpendError::SourceWalletNotFound))),
			version => panic!("test does not define memo support for ledger {version}"),
		}

		// The missing-wallet error must not poison the context mutex.
		assert!(context.try_with_wallet_from_seed(test_seed(), |_| ()).is_none());
	}

	#[test]
	fn selecting_from_an_unregistered_source_wallet_returns_a_typed_error() {
		let context = Arc::new(LedgerContext::<DefaultDB>::new("test"));
		let result = InputInfo::coins_to_cover_value(
			context.clone(),
			test_seed(),
			1,
			test_token_type(),
			CoinSelectionStrategy::LargestFirst,
		);

		assert!(matches!(result, Err(ShieldedCoinSelectionError::SourceWalletNotFound)));
		// The selection failure likewise leaves the context usable.
		assert!(context.try_with_wallet_from_seed(test_seed(), |_| ()).is_none());
	}

	#[test]
	fn missing_exact_nullifier_returns_a_distinct_typed_coin_error() {
		let context =
			Arc::new(LedgerContext::<DefaultDB>::new_from_wallet_seeds("test", &[test_seed()]));
		let mut input = make_input(1);
		input.nullifier = Some(Nullifier(HashOutput([0x24; 32])));
		input.memo = Some(vec![0x42]);
		let mut rng = StdRng::from_seed([0u8; 32]);
		let result: Result<Input<ProofPreimage, DefaultDB>, crate::ShieldedSpendError> =
			input.build(&mut rng, context);

		match LEDGER_VERSION {
			7 | 8 => assert!(matches!(
				result,
				Err(crate::ShieldedSpendError::MemoUnsupportedLedger { version })
					if version == LEDGER_VERSION
			)),
			9 => assert!(matches!(
				result,
				Err(crate::ShieldedSpendError::ShieldedCoinNotFoundByNullifier)
			)),
			version => panic!("test does not define memo support for ledger {version}"),
		}
	}

	#[test]
	fn select_inputs_exact_match() {
		let inputs = vec![make_input(100)];
		let result = InputInfo::select_inputs(inputs, 100, CoinSelectionStrategy::LargestFirst);
		let (selected, change) = result.expect("should select inputs");
		assert_eq!(selected.len(), 1);
		assert_eq!(change, 0);
	}

	#[test]
	fn select_inputs_multiple_sum_to_required() {
		let inputs = vec![make_input(60), make_input(40)];
		let result = InputInfo::select_inputs(inputs, 100, CoinSelectionStrategy::LargestFirst);
		let (selected, change) = result.expect("should select inputs");
		assert_eq!(selected.len(), 2);
		assert_eq!(change, 0);
	}

	#[test]
	fn select_inputs_change_produced() {
		let inputs = vec![make_input(150)];
		let result = InputInfo::select_inputs(inputs, 100, CoinSelectionStrategy::LargestFirst);
		let (selected, change) = result.expect("should select inputs");
		assert_eq!(selected.len(), 1);
		assert_eq!(change, 50);
	}

	#[test]
	fn select_inputs_accumulation_overflow_returns_none() {
		let half_plus_one = u128::MAX / 2 + 1;
		let inputs = vec![make_input(half_plus_one), make_input(half_plus_one)];
		let result =
			InputInfo::select_inputs(inputs, u128::MAX, CoinSelectionStrategy::LargestFirst);
		assert!(result.is_none(), "accumulation overflow should return None");
	}

	#[test]
	fn select_inputs_overflow_with_remaining_inputs_returns_none() {
		// After two inputs the accumulator overflows; the remaining input must not
		// cause a panic, and the call must return None.
		let large = u128::MAX / 2 + 1;
		let inputs = vec![make_input(large), make_input(large), make_input(large)];
		let result =
			InputInfo::select_inputs(inputs, u128::MAX, CoinSelectionStrategy::LargestFirst);
		assert!(result.is_none());
	}

	#[test]
	fn select_inputs_zero_required() {
		let inputs = vec![make_input(50)];
		let result = InputInfo::select_inputs(inputs, 0, CoinSelectionStrategy::LargestFirst);
		let (selected, change) = result.expect("zero required should select first input");
		assert_eq!(selected.len(), 1);
		assert_eq!(change, 50);
	}

	#[test]
	fn select_inputs_insufficient_returns_none() {
		let inputs = vec![make_input(30), make_input(20)];
		let result = InputInfo::select_inputs(inputs, 100, CoinSelectionStrategy::LargestFirst);
		assert!(result.is_none(), "insufficient inputs should return None");
	}

	#[test]
	fn select_inputs_largest_first_minimizes_count() {
		let inputs = vec![make_input(10), make_input(20), make_input(100)];
		let result = InputInfo::select_inputs(inputs, 25, CoinSelectionStrategy::LargestFirst);
		let (selected, change) = result.expect("should select inputs");
		assert_eq!(selected.len(), 1);
		assert_eq!(selected[0].value, 100);
		assert_eq!(change, 75);
	}

	#[test]
	fn select_inputs_smallest_first_consolidates_dust() {
		let inputs = vec![make_input(10), make_input(20), make_input(100)];
		let result = InputInfo::select_inputs(inputs, 25, CoinSelectionStrategy::SmallestFirst);
		let (selected, change) = result.expect("should select inputs");
		assert_eq!(selected.len(), 2);
		assert_eq!(selected[0].value, 10);
		assert_eq!(selected[1].value, 20);
		assert_eq!(change, 5);
	}
}
