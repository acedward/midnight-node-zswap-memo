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

// Resolved ledger versions, baked in at compile time by `build.rs` from `Cargo.lock`
// (`cargo metadata --locked`). This replaces the runtime `Cargo.toml` parse the
// LeastAuthority audit flagged ("Prefer Cargo.lock For Build-Time Crate Versions"): the constants
// reflect what was actually resolved and built, and git deps carry their locked tag + commit SHA.
const LEDGER_7_VERSION: &str = env!("LEDGER_7_VERSION");
const LEDGER_8_VERSION: &str = env!("LEDGER_8_VERSION");
const LEDGER_9_VERSION: &str = env!("LEDGER_9_VERSION");

/// Resolved version of a workspace ledger dependency alias, embedded at compile time from
/// `Cargo.lock`.
///
/// Registry deps return the bare semver (`"7.0.3"`). Git deps append the locked pin, in one of
/// two shapes depending on how the manifest points at the repository:
///
/// - via a mutable ref:
///   `"1.0.0 (tag: crate-ledger-9.1.0.0-rc.3, rev: 85e769a0e352518c979cb6f7a07901b63e1c124d)"`
/// - via a commit directly:
///   `"1.0.0 (rev: 6bd23733bb02f136d557e75b10eb1d4a59b89dc2)"`
///
/// Both carry the locked commit, which is the immutable build identity; only the first also has a
/// tag or branch, which is context and can be moved. The regression tests parse and validate both
/// forms rather than assuming a mutable ref is present.
/// Returns `None` for an unknown alias.
pub fn find_dependency_version(alias: &str) -> Option<String> {
	match alias {
		"mn-ledger" => Some(LEDGER_7_VERSION.to_owned()),
		"mn-ledger-8" => Some(LEDGER_8_VERSION.to_owned()),
		"mn-ledger-9" => Some(LEDGER_9_VERSION.to_owned()),
		_ => None,
	}
}

/// The pin `build.rs` recorded for a git dependency.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub struct GitPin<'a> {
	/// The mutable ref cargo was pointed at, as `(kind, name)` — `("tag", "…")` or
	/// `("branch", "…")`.
	///
	/// `None` when the manifest pinned a commit directly with `rev = "…"`. That is the *stronger*
	/// form, not a weaker one: there is no ref left that could be moved out from under the build.
	pub mutable_ref: Option<(&'a str, &'a str)>,
	/// The locked commit. Immutable, and the only field that identifies what was actually built.
	pub commit: &'a str,
}

/// Parses the annotation [`find_dependency_version`] attaches to a git dependency.
///
/// Returns `None` for registry and path deps, which are a bare semver with no annotation.
#[cfg(test)]
pub fn parse_git_pin(version: &str) -> Option<GitPin<'_>> {
	let inner = version.split_once(" (")?.1.strip_suffix(')')?;
	// `tag`/`branch` pins are `"<kind>: <name>, rev: <sha>"`; a direct commit pin is just
	// `"rev: <sha>"`. Git refs cannot contain whitespace, so `", "` unambiguously separates them.
	let (mutable_ref, rev) = match inner.split_once(", ") {
		Some((label, rev)) => {
			let (kind, name) = label.split_once(": ")?;
			if !matches!(kind, "tag" | "branch") || name.is_empty() {
				return None;
			}
			(Some((kind, name)), rev)
		},
		None => (None, inner),
	};
	let commit = rev.strip_prefix("rev: ")?;
	if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
		return None;
	}
	Some(GitPin { mutable_ref, commit })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn resolves_registry_version_without_comparator() {
		// The lock-resolved bare semver, not the `=7.0.3` manifest spec.
		let v = find_dependency_version("mn-ledger").expect("mn-ledger should resolve");
		assert!(!v.starts_with('='), "expected resolved version, got {v:?}");
		assert!(v.starts_with(|c: char| c.is_ascii_digit()), "got {v:?}");
	}

	#[test]
	fn disambiguates_same_crate_at_different_versions() {
		// `mn-ledger` and `mn-ledger-8` both rename `midnight-ledger`; the resolve graph keeps them
		// distinct, so they must report different versions.
		assert!(find_dependency_version("mn-ledger").unwrap().starts_with("7."));
		assert!(find_dependency_version("mn-ledger-8").unwrap().starts_with("8."));
	}

	#[test]
	fn annotates_git_dependency_with_its_immutable_commit() {
		// What has to hold is that a git dep is identified by the commit that was actually built.
		// A tag or branch is optional context: `rev = "<sha>"` in the manifest is the *stronger*
		// pin, because there is no ref left to move. Requiring `tag:` outright would fail exactly
		// the deps that are pinned hardest - which is what it used to do here, since
		// `midnight-ledger-v9` is pinned by rev.
		let v = find_dependency_version("mn-ledger-9").expect("mn-ledger-9 should resolve");
		let pin =
			parse_git_pin(&v).unwrap_or_else(|| panic!("expected a git pin annotation, got {v:?}"));
		assert_eq!(
			pin.commit.len(),
			40,
			"expected a full 40-char commit sha, got {:?} in {v:?}",
			pin.commit
		);
		assert!(
			pin.commit.chars().all(|c| c.is_ascii_hexdigit()),
			"commit is not hex: {:?} in {v:?}",
			pin.commit
		);
		if let Some((kind, name)) = pin.mutable_ref {
			assert!(matches!(kind, "tag" | "branch"), "unknown ref kind {kind:?} in {v:?}");
			assert!(!name.is_empty(), "empty ref name in {v:?}");
		}
	}

	#[test]
	fn parses_every_pin_shape_build_rs_can_emit() {
		// Covers the shapes the *other* aliases and any future re-pin can produce, so switching
		// `midnight-ledger-v9` between a tag and a rev cannot quietly break the assertion above.
		assert_eq!(parse_git_pin("7.0.3"), None, "registry deps carry no annotation");
		assert_eq!(
			parse_git_pin(
				"1.0.0 (tag: crate-ledger-9.1.0.0-rc.3, rev: 85e769a0e352518c979cb6f7a07901b63e1c124d)"
			),
			Some(GitPin {
				mutable_ref: Some(("tag", "crate-ledger-9.1.0.0-rc.3")),
				commit: "85e769a0e352518c979cb6f7a07901b63e1c124d",
			})
		);
		assert_eq!(
			parse_git_pin("1.0.0 (branch: main, rev: 6bd23733bb02f136d557e75b10eb1d4a59b89dc2)"),
			Some(GitPin {
				mutable_ref: Some(("branch", "main")),
				commit: "6bd23733bb02f136d557e75b10eb1d4a59b89dc2",
			})
		);
		assert_eq!(
			parse_git_pin("1.0.0 (rev: 6bd23733bb02f136d557e75b10eb1d4a59b89dc2)"),
			Some(GitPin { mutable_ref: None, commit: "6bd23733bb02f136d557e75b10eb1d4a59b89dc2" })
		);
		// Malformed annotations must not parse into a pin that looks trustworthy.
		assert_eq!(parse_git_pin("1.0.0 (tag: only-a-tag)"), None);
		assert_eq!(parse_git_pin("1.0.0 (rev: abc"), None);
		assert_eq!(parse_git_pin("1.0.0 (rev: abc)"), None, "short rev");
		assert_eq!(
			parse_git_pin("1.0.0 (rev: gggggggggggggggggggggggggggggggggggggggg)"),
			None,
			"non-hex rev"
		);
		assert_eq!(
			parse_git_pin("1.0.0 (ref: main, rev: 6bd23733bb02f136d557e75b10eb1d4a59b89dc2)"),
			None,
			"unknown label"
		);
		assert_eq!(
			parse_git_pin("1.0.0 (garbage, rev: 6bd23733bb02f136d557e75b10eb1d4a59b89dc2)"),
			None,
			"malformed label"
		);
		assert_eq!(
			parse_git_pin("1.0.0 (tag: , rev: 6bd23733bb02f136d557e75b10eb1d4a59b89dc2)"),
			None,
			"empty label"
		);
	}

	#[test]
	fn returns_none_for_missing_crate() {
		assert_eq!(find_dependency_version("mn-ldgr"), None);
	}
}
