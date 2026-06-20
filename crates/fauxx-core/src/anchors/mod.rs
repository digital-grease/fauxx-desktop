// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Account-anchor scanner (C3 #19, D5c).
//!
//! Part of the lawful "deterministic-channel defense": help the user SEE how
//! their real accounts anchor and link their identity, then recommend how to
//! partition them. This is the high-leverage identity-layer analysis the phone
//! cannot do.
//!
//! ## HARD GUARDRAIL: read-only analysis only
//!
//! This module is READ-ONLY analysis over a USER-CURATED inventory. It NEVER
//! scrapes, logs into, or automates against any real account: the user types in
//! what accounts they have and which identity signals each anchors, and the
//! scanner computes scores and recommendations from that inventory alone. There
//! is no browser, no network, no credential. The type system enforces this:
//! there is no method anywhere in this module that takes a credential, drives a
//! browser, or performs I/O against an account. A unit test asserts the
//! analysis surface is pure (it operates only on the in-memory inventory).
//!
//! ## What it produces
//!
//! - An INVENTORY of [`AccountAnchor`] records, each naming an account and the
//!   identity [`IdentitySignal`]s it anchors (verified email, phone, legal
//!   name, payment, recovery contacts).
//! - A SCORE per anchor ([`anchor_score`]) capturing how strongly and how
//!   broadly it links identity across the inventory (a documented heuristic).
//! - Prioritized [`Recommendation`]s ordered by linkage strength: separate
//!   aliases (via D3c), split recovery contacts, isolate high-anchor accounts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// A single identity signal an account can anchor. Each signal carries an
/// intrinsic linkage weight: how strongly its presence ties an account to the
/// real-world person. The weights are the documented scoring heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentitySignal {
    /// A verified email address on the account.
    VerifiedEmail,
    /// A phone number on the account (verification / 2FA / recovery).
    PhoneNumber,
    /// The user's real legal name on the account.
    LegalName,
    /// A payment instrument (card / bank) on the account.
    Payment,
    /// A recovery contact (recovery email or phone, or a security-question set)
    /// that bridges this account to another.
    RecoveryContact,
}

impl IdentitySignal {
    /// Every identity signal, in declaration order.
    pub const ALL: &'static [IdentitySignal] = &[
        IdentitySignal::VerifiedEmail,
        IdentitySignal::PhoneNumber,
        IdentitySignal::LegalName,
        IdentitySignal::Payment,
        IdentitySignal::RecoveryContact,
    ];

    /// The stable persistence/wire string for this signal.
    pub fn as_str(&self) -> &'static str {
        match self {
            IdentitySignal::VerifiedEmail => "verified-email",
            IdentitySignal::PhoneNumber => "phone-number",
            IdentitySignal::LegalName => "legal-name",
            IdentitySignal::Payment => "payment",
            IdentitySignal::RecoveryContact => "recovery-contact",
        }
    }

    /// Parse the stored string form, failing closed on an unknown value.
    pub fn from_str_strict(s: &str) -> Result<Self> {
        match s {
            "verified-email" => Ok(IdentitySignal::VerifiedEmail),
            "phone-number" => Ok(IdentitySignal::PhoneNumber),
            "legal-name" => Ok(IdentitySignal::LegalName),
            "payment" => Ok(IdentitySignal::Payment),
            "recovery-contact" => Ok(IdentitySignal::RecoveryContact),
            other => Err(CoreError::Anchor(format!(
                "unknown identity signal {other:?}"
            ))),
        }
    }

    /// The intrinsic linkage weight of this signal (the scoring heuristic).
    ///
    /// Stronger real-world identifiers weigh more: a legal name or a payment
    /// instrument ties an account to the person far more firmly than a phone
    /// number, which is stronger than a single email. A recovery contact weighs
    /// heavily because it actively BRIDGES accounts (its cross-account linkage
    /// is scored separately too, in [`anchor_score`]).
    pub fn weight(&self) -> u32 {
        match self {
            IdentitySignal::LegalName => 5,
            IdentitySignal::Payment => 5,
            IdentitySignal::RecoveryContact => 4,
            IdentitySignal::PhoneNumber => 3,
            IdentitySignal::VerifiedEmail => 2,
        }
    }
}

/// A user-curated account-anchor record (C3 #19).
///
/// One row per real account the user chooses to inventory. Persisted in the
/// `account_anchors` table; the [`crate::store::EncryptedStore`] round-trips
/// this whole record. Everything here is typed in BY THE USER; nothing is
/// scraped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAnchor {
    /// Stable id for this anchor (UUID v4 string).
    pub id: String,
    /// A human label for the account (e.g. "Personal Gmail", "Bank X").
    pub label: String,
    /// The site/service host or label (e.g. "google.com").
    pub site: String,
    /// The identity signals this account anchors, deduplicated and sorted for
    /// a deterministic record.
    pub signals: Vec<IdentitySignal>,
    /// Optional shared-contact key: a stable token for a contact value shared
    /// across accounts (e.g. a recovery email or phone), used to detect
    /// cross-account linkage WITHOUT storing the raw contact. Two anchors with
    /// the same `shared_contact_key` are linked through that contact. `None`
    /// when the account shares no curated contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_contact_key: Option<String>,
    /// Epoch millis the anchor was added to the inventory.
    pub created_at: i64,
}

impl AccountAnchor {
    /// Build an anchor, deduplicating and sorting the signal set for a
    /// deterministic record.
    pub fn new(
        id: String,
        label: &str,
        site: &str,
        signals: impl IntoIterator<Item = IdentitySignal>,
        shared_contact_key: Option<String>,
        created_at: i64,
    ) -> Self {
        let mut signals: Vec<IdentitySignal> = signals.into_iter().collect();
        signals.sort();
        signals.dedup();
        Self {
            id,
            label: label.to_string(),
            site: site.to_string(),
            signals,
            shared_contact_key,
            created_at,
        }
    }

    /// The sum of intrinsic signal weights this account carries (the
    /// "strength" half of the heuristic, before cross-account breadth).
    pub fn intrinsic_strength(&self) -> u32 {
        self.signals.iter().map(|s| s.weight()).sum()
    }
}

/// A computed anchor score for one account, with the components that produced
/// it so a client can explain the ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorScore {
    /// The anchor id this score is for.
    pub anchor_id: String,
    /// The account label, copied for display.
    pub label: String,
    /// The intrinsic signal strength (sum of signal weights).
    pub strength: u32,
    /// How many OTHER accounts in the inventory this one links to through a
    /// shared contact (the "breadth" half of the heuristic).
    pub linked_accounts: u32,
    /// The combined linkage score: strength scaled up by how broadly the
    /// account links across the inventory. Higher = a more dangerous anchor.
    pub score: u32,
}

/// Per-signal bonus added once for each OTHER account a shared recovery contact
/// reaches. Bridging contacts are the highest-leverage linkage, so a shared
/// contact that touches many accounts compounds the score.
const LINK_BONUS_PER_ACCOUNT: u32 = 4;

/// Compute the [`AnchorScore`] for `anchor` given the whole `inventory`.
///
/// The documented heuristic, in two parts:
///
/// 1. STRENGTH: the sum of the anchor's signal weights
///    ([`AccountAnchor::intrinsic_strength`]). Strong real identifiers (legal
///    name, payment) weigh most.
/// 2. BREADTH: the number of OTHER accounts that share this anchor's
///    `shared_contact_key`. Each linked account adds `LINK_BONUS_PER_ACCOUNT`
///    to the score, because a contact that bridges many accounts links identity
///    far more broadly than an isolated one.
///
/// `score = strength + linked_accounts * LINK_BONUS_PER_ACCOUNT`.
///
/// Pure: operates only on the in-memory inventory. No I/O, no network, no
/// account access.
pub fn anchor_score(anchor: &AccountAnchor, inventory: &[AccountAnchor]) -> AnchorScore {
    let strength = anchor.intrinsic_strength();

    let linked_accounts = match &anchor.shared_contact_key {
        Some(key) => inventory
            .iter()
            .filter(|other| {
                other.id != anchor.id && other.shared_contact_key.as_deref() == Some(key)
            })
            .count() as u32,
        None => 0,
    };

    let score = strength + linked_accounts * LINK_BONUS_PER_ACCOUNT;

    AnchorScore {
        anchor_id: anchor.id.clone(),
        label: anchor.label.clone(),
        strength,
        linked_accounts,
        score,
    }
}

/// Score every anchor in the inventory, returned highest-score first (ties
/// broken by label for a stable order). Pure analysis over the inventory.
pub fn score_inventory(inventory: &[AccountAnchor]) -> Vec<AnchorScore> {
    let mut scores: Vec<AnchorScore> = inventory
        .iter()
        .map(|a| anchor_score(a, inventory))
        .collect();
    scores.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.anchor_id.cmp(&b.anchor_id))
    });
    scores
}

/// The kind of partitioning action a [`Recommendation`] proposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecommendationKind {
    /// Give this account its own masked alias / plus-address (via D3c) instead
    /// of a shared real email.
    SeparateAlias,
    /// Stop using a shared recovery contact that bridges this account to others.
    SplitRecoveryContact,
    /// Isolate this high-anchor account (move it onto its own identity surface).
    IsolateHighAnchor,
}

impl RecommendationKind {
    /// The stable persistence/wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            RecommendationKind::SeparateAlias => "separate-alias",
            RecommendationKind::SplitRecoveryContact => "split-recovery-contact",
            RecommendationKind::IsolateHighAnchor => "isolate-high-anchor",
        }
    }
}

/// A prioritized partitioning recommendation for one account (C3 #19).
///
/// Advice only: it names what to do and why, ordered by linkage strength.
/// It NEVER acts on a real account; acting on it (e.g. minting an alias) is a
/// separate, user-driven D3c step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    /// The anchor this recommendation targets.
    pub anchor_id: String,
    /// The account label, copied for display.
    pub label: String,
    /// What to do.
    pub kind: RecommendationKind,
    /// The anchor's linkage score that ranked this recommendation.
    pub score: u32,
    /// A human-readable rationale.
    pub rationale: String,
}

/// The strength score at or above which an account is considered a "high
/// anchor" worth isolating outright.
const HIGH_ANCHOR_STRENGTH: u32 = 8;

/// Produce prioritized partitioning recommendations for the inventory, ordered
/// by linkage strength (highest score first). For each account, in priority
/// order:
///
/// 1. If a shared recovery contact bridges it to other accounts, recommend
///    SPLITTING that recovery contact (the highest-leverage fix).
/// 2. If it carries a verified email signal, recommend a SEPARATE ALIAS so the
///    address fronting it is not reused (feeds D3c).
/// 3. If its intrinsic strength is high, recommend ISOLATING it.
///
/// The returned list is sorted by the anchor's score descending so the most
/// dangerous anchors surface first. Pure analysis; never acts on an account.
pub fn recommendations(inventory: &[AccountAnchor]) -> Vec<Recommendation> {
    // Index scores by anchor id for the ordering and rationale.
    let scores = score_inventory(inventory);
    let score_by_id: BTreeMap<&str, &AnchorScore> =
        scores.iter().map(|s| (s.anchor_id.as_str(), s)).collect();

    let mut recs: Vec<Recommendation> = Vec::new();

    for anchor in inventory {
        let score = match score_by_id.get(anchor.id.as_str()) {
            Some(s) => *s,
            None => continue,
        };

        // 1) Shared recovery contact bridging to other accounts.
        if score.linked_accounts > 0 && anchor.signals.contains(&IdentitySignal::RecoveryContact) {
            recs.push(Recommendation {
                anchor_id: anchor.id.clone(),
                label: anchor.label.clone(),
                kind: RecommendationKind::SplitRecoveryContact,
                score: score.score,
                rationale: format!(
                    "A shared recovery contact links this account to {} other account(s); \
                     splitting it breaks the strongest cross-account linkage.",
                    score.linked_accounts
                ),
            });
        }

        // 2) Verified email -> recommend a per-site alias.
        if anchor.signals.contains(&IdentitySignal::VerifiedEmail) {
            recs.push(Recommendation {
                anchor_id: anchor.id.clone(),
                label: anchor.label.clone(),
                kind: RecommendationKind::SeparateAlias,
                score: score.score,
                rationale: "Front this account with its own masked alias or plus-address so the \
                     email does not link it to your other accounts."
                    .to_string(),
            });
        }

        // 3) High intrinsic strength -> isolate it.
        if anchor.intrinsic_strength() >= HIGH_ANCHOR_STRENGTH {
            recs.push(Recommendation {
                anchor_id: anchor.id.clone(),
                label: anchor.label.clone(),
                kind: RecommendationKind::IsolateHighAnchor,
                score: score.score,
                rationale: "This account carries strong identifiers (legal name and/or payment); \
                     isolate it onto its own identity surface to limit its blast radius."
                    .to_string(),
            });
        }
    }

    // Order by linkage strength (score) descending, with a stable tiebreak so
    // the same inventory always yields the same ordered list.
    recs.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
    });
    recs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(
        id: &str,
        label: &str,
        signals: &[IdentitySignal],
        shared: Option<&str>,
    ) -> AccountAnchor {
        AccountAnchor::new(
            id.to_string(),
            label,
            "site.test",
            signals.iter().copied(),
            shared.map(str::to_string),
            1_700_000_000_000,
        )
    }

    #[test]
    fn signal_strings_round_trip() -> Result<()> {
        for s in IdentitySignal::ALL {
            assert_eq!(IdentitySignal::from_str_strict(s.as_str())?, *s);
        }
        assert!(matches!(
            IdentitySignal::from_str_strict("nope"),
            Err(CoreError::Anchor(_))
        ));
        Ok(())
    }

    #[test]
    fn new_anchor_dedups_and_sorts_signals() {
        let a = anchor(
            "a",
            "Acct",
            &[
                IdentitySignal::PhoneNumber,
                IdentitySignal::VerifiedEmail,
                IdentitySignal::PhoneNumber, // dup
            ],
            None,
        );
        assert_eq!(
            a.signals,
            vec![IdentitySignal::VerifiedEmail, IdentitySignal::PhoneNumber]
        );
    }

    #[test]
    fn intrinsic_strength_sums_weights() {
        let a = anchor(
            "a",
            "Bank",
            &[IdentitySignal::LegalName, IdentitySignal::Payment],
            None,
        );
        assert_eq!(a.intrinsic_strength(), 10); // 5 + 5
    }

    #[test]
    fn score_combines_strength_and_breadth() {
        // Three accounts share recovery contact "rk"; one does not.
        let inv = vec![
            anchor(
                "a",
                "Hub",
                &[IdentitySignal::LegalName, IdentitySignal::RecoveryContact],
                Some("rk"),
            ),
            anchor("b", "Spoke1", &[IdentitySignal::VerifiedEmail], Some("rk")),
            anchor("c", "Spoke2", &[IdentitySignal::VerifiedEmail], Some("rk")),
            anchor("d", "Lonely", &[IdentitySignal::VerifiedEmail], None),
        ];
        let hub = anchor_score(&inv[0], &inv);
        // strength = 5 (legal name) + 4 (recovery) = 9; linked to 2 others.
        assert_eq!(hub.strength, 9);
        assert_eq!(hub.linked_accounts, 2);
        assert_eq!(hub.score, 9 + 2 * LINK_BONUS_PER_ACCOUNT);

        let lonely = anchor_score(&inv[3], &inv);
        assert_eq!(lonely.linked_accounts, 0);
        assert_eq!(lonely.score, 2);
    }

    #[test]
    fn score_inventory_orders_highest_first() {
        let inv = vec![
            anchor("a", "Weak", &[IdentitySignal::VerifiedEmail], None),
            anchor(
                "b",
                "Strong",
                &[IdentitySignal::LegalName, IdentitySignal::Payment],
                None,
            ),
            anchor("c", "Medium", &[IdentitySignal::PhoneNumber], None),
        ];
        let scored = score_inventory(&inv);
        assert_eq!(scored[0].label, "Strong"); // 10
        assert_eq!(scored[1].label, "Medium"); // 3
        assert_eq!(scored[2].label, "Weak"); // 2
                                             // Monotonic non-increasing.
        assert!(scored[0].score >= scored[1].score);
        assert!(scored[1].score >= scored[2].score);
    }

    #[test]
    fn recommendations_prioritize_by_linkage_strength() {
        let inv = vec![
            // High anchor bridging two spokes: should top the list.
            anchor(
                "hub",
                "Primary Email",
                &[
                    IdentitySignal::LegalName,
                    IdentitySignal::VerifiedEmail,
                    IdentitySignal::RecoveryContact,
                ],
                Some("rk"),
            ),
            anchor("s1", "Shop A", &[IdentitySignal::VerifiedEmail], Some("rk")),
            anchor("s2", "Shop B", &[IdentitySignal::VerifiedEmail], Some("rk")),
        ];
        let recs = recommendations(&inv);
        assert!(!recs.is_empty());
        // Ordered by score descending.
        for w in recs.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
        // The bridging hub gets a split-recovery-contact recommendation.
        assert!(recs
            .iter()
            .any(|r| r.anchor_id == "hub" && r.kind == RecommendationKind::SplitRecoveryContact));
        // Every verified-email account gets a separate-alias recommendation.
        assert!(recs
            .iter()
            .any(|r| r.anchor_id == "s1" && r.kind == RecommendationKind::SeparateAlias));
        // The top recommendation targets the highest-scoring (hub) account.
        assert_eq!(recs[0].anchor_id, "hub");
    }

    #[test]
    fn high_anchor_gets_isolation_recommendation() {
        let inv = vec![anchor(
            "bank",
            "Bank",
            &[IdentitySignal::LegalName, IdentitySignal::Payment],
            None,
        )];
        let recs = recommendations(&inv);
        assert!(recs
            .iter()
            .any(|r| r.kind == RecommendationKind::IsolateHighAnchor));
    }

    #[test]
    fn empty_inventory_yields_no_recommendations() {
        assert!(recommendations(&[]).is_empty());
        assert!(score_inventory(&[]).is_empty());
    }

    /// The analysis surface is PURE: scoring and recommendations are functions
    /// of the in-memory inventory only. This is the read-only / no-automation
    /// property as a compile-and-run assertion: re-running the analysis any
    /// number of times never changes the inventory and produces identical
    /// output (no hidden state, no side effects, no account access).
    #[test]
    fn analysis_is_pure_and_read_only() {
        let inv = vec![
            anchor(
                "hub",
                "Primary",
                &[IdentitySignal::LegalName, IdentitySignal::RecoveryContact],
                Some("rk"),
            ),
            anchor("s1", "Spoke", &[IdentitySignal::VerifiedEmail], Some("rk")),
        ];
        let before = inv.clone();
        let r1 = recommendations(&inv);
        let s1 = score_inventory(&inv);
        let r2 = recommendations(&inv);
        let s2 = score_inventory(&inv);
        // Inventory is untouched by analysis (read-only).
        assert_eq!(inv, before);
        // Deterministic / no hidden state.
        assert_eq!(r1, r2);
        assert_eq!(s1, s2);
    }
}
