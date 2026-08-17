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

use std::str::FromStr;

use midnight_node_ledger_helpers::*;
use serde::Deserialize;

use crate::tx_generator::source::FetchCacheConfig;

pub trait TokenDecode: Sized + Send + Sync + Clone {
	fn decode(token_id: [u8; 32]) -> Self;
}

impl TokenDecode for UnshieldedTokenType {
	fn decode(token_id: [u8; 32]) -> Self {
		UnshieldedTokenType(HashOutput(token_id))
	}
}

impl TokenDecode for ShieldedTokenType {
	fn decode(token_id: [u8; 32]) -> Self {
		ShieldedTokenType(HashOutput(token_id))
	}
}

pub fn token_decode<T: TokenDecode>(input: &str) -> Result<T, clap::error::Error> {
	let token_id: [u8; 32] = hex_str_decode(input)?;
	let token = T::decode(token_id);

	Ok(token)
}

pub fn coin_selection_strategy(input: &str) -> Result<CoinSelectionStrategy, clap::error::Error> {
	match input {
		"largest-first" => Ok(CoinSelectionStrategy::LargestFirst),
		"smallest-first" => Ok(CoinSelectionStrategy::SmallestFirst),
		other => {
			let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
			err.insert(
				clap::error::ContextKind::Custom,
				clap::error::ContextValue::String(format!(
					"invalid coin selection strategy '{}': expected 'largest-first' or 'smallest-first'",
					other
				)),
			);
			Err(err)
		},
	}
}

pub fn wallet_seed_decode(input: &str) -> Result<WalletSeed, clap::error::Error> {
	input.parse().map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to parse seed: {}", e)),
		);
		err
	})
}

/// A NIGHT wallet seed together with its [`UnshieldedSignatureScheme`], parsed from a single CLI
/// (or JSON) value: `[schnorr:|ecdsa:]<hex|lazy-hex|mnemonic>`. No prefix defaults to Schnorr —
/// backwards compatible with the historical bare `--seed <seed>` form. An explicit `ecdsa:`
/// prefix selects the ledger-9+ ECDSA scheme.
#[derive(Clone, Debug)]
pub struct SchemeSeed {
	pub seed: WalletSeed,
	pub scheme: UnshieldedSignatureScheme,
}

impl SchemeSeed {
	/// The seed and its unshielded signature scheme, as a pair.
	pub fn resolve(&self) -> (WalletSeed, UnshieldedSignatureScheme) {
		(self.seed.clone(), self.scheme)
	}
}

#[derive(Debug, thiserror::Error)]
pub enum SchemeSeedParseError {
	#[error("unknown seed scheme '{0}', expected 'schnorr' or 'ecdsa'")]
	UnknownScheme(String),
	#[error(transparent)]
	Seed(#[from] WalletSeedParseError),
}

impl FromStr for SchemeSeed {
	type Err = SchemeSeedParseError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let (scheme, rest) = match s.split_once(':') {
			Some(("schnorr", rest)) => (UnshieldedSignatureScheme::Schnorr, rest),
			Some(("ecdsa", rest)) => (UnshieldedSignatureScheme::Ecdsa, rest),
			Some((prefix, _)) => return Err(SchemeSeedParseError::UnknownScheme(prefix.into())),
			None => (UnshieldedSignatureScheme::Schnorr, s),
		};
		Ok(SchemeSeed { seed: rest.parse()?, scheme })
	}
}

impl<'de> serde::Deserialize<'de> for SchemeSeed {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let s = <String as serde::Deserialize>::deserialize(deserializer)?;
		s.parse().map_err(serde::de::Error::custom)
	}
}

/// Clap `value_parser` adapter for [`SchemeSeed`].
pub fn scheme_seed_decode(input: &str) -> Result<SchemeSeed, clap::error::Error> {
	input.parse().map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to parse seed: {}", e)),
		);
		err
	})
}

pub fn keypair_from_str(input: &str) -> Result<Keypair, clap::error::Error> {
	input.parse().map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to parse keypair: {}", e)),
		);
		err
	})
}

pub fn serde_json_decode<T: for<'a> Deserialize<'a>>(input: &str) -> Result<T, clap::error::Error> {
	serde_json::from_str(input).map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to parse input json: {}", e)),
		);
		err
	})
}

pub fn hex_ledger_decode<T: Deserializable + Tagged>(input: &str) -> Result<T, clap::error::Error> {
	hex_ledger_tagged_decode::<T>(input)
}

// ADR-0022: wallet keys and addresses (including contract addresses and coin
// public keys) use *untagged* serialization. They are surfaced to users as
// Bech32m, where the human-readable-part already plays the role of a tag.
// Switching to `hex_ledger_decode` (tagged) was tried and reverted in PR #853;
// do not re-introduce it without first updating ADR-0022. EOF enforcement in
// `hex_ledger_untagged_decode` is the audit-#307 hardening that closes the
// silent-fallback ambiguity surface without changing the wire format.
pub fn coin_public_decode(input: &str) -> Result<CoinPublicKey, clap::error::Error> {
	hex_ledger_untagged_decode(input)
}

// ADR-0022: see the comment on `coin_public_decode`. `ContractAddress` is in
// the same untagged set; switching to tagged decoding was reverted in PR #853
// and must not be re-introduced without first updating ADR-0022.
pub fn contract_address_decode(input: &str) -> Result<ContractAddress, clap::error::Error> {
	hex_ledger_untagged_decode(input)
}

pub fn hex_ledger_untagged_decode<T>(input: &str) -> Result<T, clap::error::Error>
where
	T: Deserializable,
{
	let bytes = hex::decode(input).map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("invalid hex input: {}", e)),
		);
		err
	})?;

	let mut cursor = &bytes[..];
	let res = <T as Deserializable>::deserialize(&mut cursor, 0).map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to deserialize arg: {e}")),
		);
		err
	})?;

	if !cursor.is_empty() {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!(
				"trailing data after deserialization: {} extra byte(s)",
				cursor.len()
			)),
		);
		return Err(err);
	}

	Ok(res)
}

pub fn hex_ledger_tagged_decode<T>(input: &str) -> Result<T, clap::error::Error>
where
	T: Deserializable + Tagged,
{
	let bytes = hex::decode(input).map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to parse: {}", e)),
		);
		err
	})?;

	let res: T = deserialize(&mut &bytes[..]).map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("failed to deserialize arg: {e}")),
		);
		err
	})?;

	Ok(res)
}

pub fn hex_bytes(input: &str) -> Result<Vec<u8>, clap::error::Error> {
	// Remove 0x prefix if present
	let hex_str = input.strip_prefix("0x").unwrap_or(input);
	hex::decode(hex_str).map_err(|e| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("invalid hex input: {}", e)),
		);
		err
	})
}

/// A zswap input memo, as raw bytes. A newtype rather than a bare `Vec<u8>` because clap reads
/// `Vec<T>` as a repeatable argument, which does not match a parser producing the whole value.
#[derive(Clone, Debug)]
pub struct MemoArg(Vec<u8>);

impl MemoArg {
	/// Checked programmatic construction. CLI parsing and direct API callers share this boundary,
	/// so constructing `SingleTxArgs` manually cannot bypass the ledger's length rule.
	pub fn new(bytes: Vec<u8>) -> Result<Self, clap::error::Error> {
		use midnight_node_ledger_helpers::latest::zswap::MAX_MEMO_BYTES;
		if bytes.is_empty() || bytes.len() > MAX_MEMO_BYTES {
			return Err(clap::Error::raw(
				clap::error::ErrorKind::ValueValidation,
				format!("memo must be 1..={MAX_MEMO_BYTES} bytes, got {}\n", bytes.len()),
			));
		}
		Ok(Self(bytes))
	}

	pub fn as_bytes(&self) -> &[u8] {
		&self.0
	}

	pub fn into_bytes(self) -> Vec<u8> {
		self.0
	}
}

impl TryFrom<Vec<u8>> for MemoArg {
	type Error = clap::error::Error;

	fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
		Self::new(bytes)
	}
}

/// Parses a hex-encoded zswap input memo, enforcing the ledger's size bounds up front so an
/// oversized memo is a CLI error rather than a failure deep in the builder.
///
/// The bound is taken from the ledger rather than restated here. A local copy would be free to
/// drift, and either direction is a bug: too small and the CLI refuses memos the chain accepts,
/// too large and the user gets a rejection from the builder after paying for coin selection.
pub fn memo_decode(input: &str) -> Result<MemoArg, clap::error::Error> {
	MemoArg::new(hex_bytes(input)?)
}

pub fn hex_str_decode<T>(input: &str) -> Result<T, clap::error::Error>
where
	T: TryFrom<Vec<u8>, Error = Vec<u8>>,
{
	let bytes = hex_bytes(input)?;
	let res: T = bytes.try_into().map_err(|e: Vec<u8>| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!(
				"incorrect length for token type string. Expected 32, got {}",
				e.len()
			)),
		);
		err
	})?;

	Ok(res)
}

pub fn fetch_cache_config(input: &str) -> Result<FetchCacheConfig, clap::Error> {
	FetchCacheConfig::from_str(input).map_err(|error| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("invalid fetch cache config: {}", error)),
		);
		err
	})
}

pub fn wallet_address(input: &str) -> Result<WalletAddress, clap::Error> {
	WalletAddress::from_str(input).map_err(|error| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("invalid wallet address: {}", error)),
		);
		err
	})
}

pub fn utxo_id_decode(input: &str) -> Result<UtxoId, clap::Error> {
	UtxoId::from_str(input).map_err(|error| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("invalid utxo id: {}", error)),
		);
		err
	})
}

// `--output` value type and parsing live in a dedicated submodule that does
// not depend on clap. The wrapper below adapts its error to `clap::Error` so
// the parser can be used as a clap `value_parser`.
pub mod output_arg;
pub use output_arg::OutputArg;

/// Clap `value_parser` adapter for `--output`. Delegates the actual parsing
/// to [`output_arg::decode`] and converts its [`output_arg::DecodeError`]
/// into a `clap::Error` for surface in the CLI.
pub fn output_arg_decode(input: &str) -> Result<OutputArg, clap::Error> {
	output_arg::decode(input).map_err(|error| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(error.to_string()),
		);
		err
	})
}

pub fn semver_decode(input: &str) -> Result<semver::Version, clap::Error> {
	semver::Version::parse(input.trim()).map_err(|error| {
		let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation);
		err.insert(
			clap::error::ContextKind::Custom,
			clap::error::ContextValue::String(format!("invalid semver: {}", error)),
		);
		err
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	// `coin_public_decode` — untagged per ADR-0022.

	#[test]
	fn coin_public_decode_accepts_untagged_input() {
		// 32-byte all-zeros payload — the untagged decoder consumes exactly 32 bytes.
		let res = coin_public_decode(&"00".repeat(32));
		assert!(res.is_ok(), "valid untagged 32-byte input should decode");
	}

	#[test]
	fn coin_public_decode_rejects_trailing_bytes() {
		// Valid 32-byte payload plus one extra byte — EOF enforcement should reject.
		let with_trailing = format!("{}00", "00".repeat(32));
		let res = coin_public_decode(&with_trailing);
		assert!(res.is_err(), "trailing bytes should be rejected (EOF enforcement)");
	}

	#[test]
	fn coin_public_decode_rejects_truncated_input() {
		// 30 bytes (60 hex chars) — short of the 32-byte payload.
		let res = coin_public_decode(&"00".repeat(30));
		assert!(res.is_err(), "truncated input should be rejected");
	}

	#[test]
	fn coin_public_decode_rejects_invalid_hex() {
		let res = coin_public_decode("not-valid-hex!!");
		assert!(res.is_err(), "invalid hex should be rejected");
	}

	// `memo_decode` — every rejection must be a typed clap error, never a panic, and the CLI must
	// agree with the ledger about which sizes exist.

	#[test]
	fn memo_decode_accepts_the_ledger_length_boundary() {
		use midnight_node_ledger_helpers::latest::zswap::MAX_MEMO_BYTES;

		// 1, both sides of the 31-byte packing chunk, and the cap.
		for len in [1usize, 31, 32, 511, MAX_MEMO_BYTES] {
			let hex = "a5".repeat(len);
			let decoded = memo_decode(&hex)
				.unwrap_or_else(|e| panic!("a {len}-byte memo must be accepted, got {e}"));
			assert_eq!(decoded.as_bytes().len(), len);
			assert!(decoded.as_bytes().iter().all(|b| *b == 0xa5), "bytes must survive verbatim");
		}
	}

	#[test]
	fn memo_decode_rejects_empty() {
		// An empty memo is not "no memo": absence is expressed by omitting `--memo` entirely, and
		// the ledger gives absence exactly one representation.
		let err = memo_decode("").expect_err("an empty memo must be rejected");
		assert!(err.to_string().contains("must be 1..="), "unhelpful error: {err}");
	}

	#[test]
	fn memo_decode_rejects_oversized() {
		use midnight_node_ledger_helpers::latest::zswap::MAX_MEMO_BYTES;

		let hex = "00".repeat(MAX_MEMO_BYTES + 1);
		let err = memo_decode(&hex).expect_err("an oversized memo must be rejected");
		let rendered = err.to_string();
		assert!(rendered.contains("must be 1..="), "unhelpful error: {rendered}");
		// The size the user actually supplied has to appear, or they cannot tell by how much they
		// overshot.
		assert!(
			rendered.contains(&(MAX_MEMO_BYTES + 1).to_string()),
			"error should report the supplied size: {rendered}"
		);
	}

	#[test]
	fn memo_decode_rejects_malformed_hex() {
		// Non-hex characters, an odd digit count, and embedded whitespace. The odd-length case is
		// the one that matters most: a decoder that dropped the trailing nibble would silently
		// produce a memo the user did not ask for, and it would still be authenticated.
		for bad in ["zz", "not-hex!!", "abc", "00 11", "0x"] {
			assert!(
				memo_decode(bad).is_err(),
				"malformed hex {bad:?} must be rejected rather than silently truncated"
			);
		}
	}

	#[test]
	fn memo_decode_accepts_an_0x_prefix() {
		// `hex_bytes` strips `0x` for every hex argument in the toolkit, so `--memo` follows the
		// same convention rather than being the one flag that refuses it.
		assert_eq!(memo_decode("0xa5").expect("0x-prefixed hex is accepted").as_bytes(), &[0xa5]);
		assert_eq!(memo_decode("a5").expect("bare hex is accepted").as_bytes(), &[0xa5]);
	}

	#[test]
	fn memo_arg_programmatic_construction_is_checked() {
		use midnight_node_ledger_helpers::latest::zswap::MAX_MEMO_BYTES;

		assert!(MemoArg::new(Vec::new()).is_err());
		assert!(MemoArg::try_from(vec![0u8; MAX_MEMO_BYTES + 1]).is_err());
		let valid = MemoArg::new(vec![0x5a; MAX_MEMO_BYTES]).expect("the cap is valid");
		assert_eq!(valid.as_bytes().len(), MAX_MEMO_BYTES);
		assert_eq!(valid.into_bytes(), vec![0x5a; MAX_MEMO_BYTES]);
	}

	#[test]
	fn memo_decode_bound_tracks_the_ledger() {
		// If the ledger's cap ever moves, this parser must move with it rather than keeping a
		// stale copy. Reading the constant is what guarantees that; this pins the value the CLI is
		// compiled against so a change is visible in review.
		assert_eq!(midnight_node_ledger_helpers::latest::zswap::MAX_MEMO_BYTES, 512);
	}

	// `contract_address_decode` — untagged per ADR-0022.

	#[test]
	fn contract_address_decode_accepts_untagged_input() {
		// Reuse the canonical untagged fixture also consumed by `generate_txs.rs`.
		let untagged_hex =
			include_str!("../../../res/test-contract/contract_address_undeployed.mn").trim();
		assert!(
			contract_address_decode(untagged_hex).is_ok(),
			"valid untagged ContractAddress hex should decode"
		);
	}

	#[test]
	fn contract_address_decode_rejects_trailing_bytes() {
		let untagged_hex =
			include_str!("../../../res/test-contract/contract_address_undeployed.mn").trim();
		let with_trailing = format!("{untagged_hex}00");
		let res = contract_address_decode(&with_trailing);
		assert!(res.is_err(), "trailing bytes should be rejected (EOF enforcement)");
	}

	#[test]
	fn contract_address_decode_rejects_truncated_input() {
		// 30 bytes — short of the 32-byte payload.
		let res = contract_address_decode(&"00".repeat(30));
		assert!(res.is_err(), "truncated input should be rejected");
	}

	#[test]
	fn contract_address_decode_rejects_invalid_hex() {
		let res = contract_address_decode("zzzz");
		assert!(res.is_err(), "invalid hex should be rejected");
	}

	// `hex_ledger_untagged_decode::<HashOutput>` — the audit-#307 EOF hardening.

	#[test]
	fn hex_ledger_untagged_decode_accepts_exact_length() {
		let res = hex_ledger_untagged_decode::<HashOutput>(&"00".repeat(32));
		assert!(res.is_ok(), "exact-length untagged input should succeed");
	}

	#[test]
	fn hex_ledger_untagged_decode_rejects_trailing_bytes() {
		// 33 bytes — one byte too many.
		let res = hex_ledger_untagged_decode::<HashOutput>(&"ab".repeat(33));
		assert!(res.is_err(), "trailing data in untagged decode should be rejected");
	}

	#[test]
	fn hex_ledger_untagged_decode_rejects_truncated_input() {
		// 30 bytes — short of the 32-byte payload.
		let res = hex_ledger_untagged_decode::<HashOutput>(&"ab".repeat(30));
		assert!(res.is_err(), "truncated input should be rejected");
	}

	// `--output` parser tests live in `cli_parsers::output_arg::tests`.

	#[test]
	fn hex_ledger_untagged_decode_rejects_invalid_hex() {
		let res = hex_ledger_untagged_decode::<HashOutput>("not-valid-hex!!");
		assert!(res.is_err(), "invalid hex should be rejected");
	}

	// `SchemeSeed::from_str` — the single Schnorr-vs-ECDSA decision point. The toolkit-js
	// `ecdsa:`-rejection guard and the whole unshielded-scheme dispatch key off it, so its
	// behaviour is pinned here: exact lowercase `schnorr:`/`ecdsa:` prefixes, a bare seed
	// defaulting to Schnorr (historical `--seed` compatibility), and any other prefix a hard
	// error — never a silent downgrade of an intended-ECDSA seed to Schnorr.

	const TEST_SEED: &str = "0000000000000000000000000000000000000000000000000000000000000037";

	#[test]
	fn scheme_seed_bare_defaults_to_schnorr() {
		let parsed: SchemeSeed = TEST_SEED.parse().expect("bare seed should parse");
		assert_eq!(parsed.scheme, UnshieldedSignatureScheme::Schnorr);
	}

	#[test]
	fn scheme_seed_schnorr_prefix() {
		let parsed: SchemeSeed = format!("schnorr:{TEST_SEED}")
			.parse()
			.expect("schnorr-prefixed seed should parse");
		assert_eq!(parsed.scheme, UnshieldedSignatureScheme::Schnorr);
		// The prefix is stripped: the resolved seed matches the bare form.
		let bare: SchemeSeed = TEST_SEED.parse().unwrap();
		assert_eq!(parsed.seed, bare.seed);
	}

	#[test]
	fn scheme_seed_ecdsa_prefix() {
		let parsed: SchemeSeed =
			format!("ecdsa:{TEST_SEED}").parse().expect("ecdsa-prefixed seed should parse");
		assert_eq!(parsed.scheme, UnshieldedSignatureScheme::Ecdsa);
		let bare: SchemeSeed = TEST_SEED.parse().unwrap();
		assert_eq!(parsed.seed, bare.seed);
	}

	#[test]
	fn scheme_seed_rejects_wrong_case_prefix_never_downgrades() {
		// The critical safety property: a mis-cased ECDSA prefix must be a hard error, NOT a
		// silent fall-through to Schnorr (which would deploy a Schnorr authority for an
		// intended-ECDSA seed). Guard against a future `to_lowercase()`/`starts_with` refactor.
		for input in [format!("ECDSA:{TEST_SEED}"), format!("Ecdsa:{TEST_SEED}")] {
			assert!(
				matches!(input.parse::<SchemeSeed>(), Err(SchemeSeedParseError::UnknownScheme(_))),
				"wrong-case ECDSA prefix must be rejected, not downgraded to Schnorr: {input}"
			);
		}
	}

	#[test]
	fn scheme_seed_rejects_unknown_and_padded_prefix() {
		for input in [
			format!("foo:{TEST_SEED}"),
			format!(" ecdsa:{TEST_SEED}"),
			format!("SCHNORR:{TEST_SEED}"),
		] {
			assert!(
				matches!(input.parse::<SchemeSeed>(), Err(SchemeSeedParseError::UnknownScheme(_))),
				"unrecognized scheme prefix must be rejected: {input}"
			);
		}
	}
}
