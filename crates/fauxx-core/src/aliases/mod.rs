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

//! Email-alias management (C3 #17, D3c).
//!
//! Part of the lawful "deterministic-channel defense": give every
//! `(persona, site)` pair its OWN email address so a leaked or sold address
//! reveals only the one site it fronts, and the linkage between accounts is
//! broken at the identity layer.
//!
//! ## What this is
//!
//! - It MINTS or RECORDS a masked alias or a plus-address scoped per
//!   `(persona, site)`. Two address kinds are supported here:
//!   - a manually-created alias the user already minted somewhere (e.g. an
//!     iCloud "Hide My Email" address) and is recording, and
//!   - a generated PLUS-ADDRESS derived from a base address
//!     (`base+tag@domain`), which needs no provider.
//! - It keeps the alias -> persona -> site inventory, supports REVOKE and
//!   ROTATE, and ENFORCES the rule that no two sites for one persona reuse the
//!   same alias unless the caller explicitly opts in.
//!
//! ## Provider seam (DEFERRED HTTP integration)
//!
//! A masking-PROVIDER API integration (e.g. an addy.io / SimpleLogin / Apple
//! Hide-My-Email backend that mints fresh forwarding addresses over HTTP) is
//! DEFERRED: the workspace declares no DIRECT `reqwest` dependency (reqwest is
//! present only transitively under chromiumoxide and is not used here), so no
//! live HTTP provider ships in this issue. The [`AliasProvider`] trait is the seam a
//! future provider impl slots into; today the only built-in provider is the
//! local, network-free [`PlusAddressProvider`]. When the HTTP provider lands,
//! its credentials go in the OS KEYSTORE (see [`crate::store::KeySource`]),
//! never in the database. The DB stores only the alias -> persona -> site
//! mapping, never a provider secret.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// How an alias address was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AliasKind {
    /// A plus-address generated locally from a base address: `base+tag@domain`.
    /// Needs no provider; the mail still lands in the base inbox.
    PlusAddress,
    /// A masked/forwarding alias the user minted out of band (e.g. an iCloud
    /// Hide-My-Email address) and is recording here. A future provider impl
    /// also produces this kind, but mints it over HTTP.
    Masked,
}

impl AliasKind {
    /// The stable persistence/wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            AliasKind::PlusAddress => "plus-address",
            AliasKind::Masked => "masked",
        }
    }

    /// Parse the stored string form, failing closed on an unknown value.
    pub fn from_str_strict(s: &str) -> Result<Self> {
        match s {
            "plus-address" => Ok(AliasKind::PlusAddress),
            "masked" => Ok(AliasKind::Masked),
            other => Err(CoreError::Alias(format!("unknown alias kind {other:?}"))),
        }
    }
}

/// The lifecycle status of an alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AliasStatus {
    /// In use, fronting its site.
    Active,
    /// Deactivated: kept for the audit trail but no longer fronting the site.
    /// A rotate marks the old alias revoked and mints a fresh active one.
    Revoked,
}

impl AliasStatus {
    /// The stable persistence/wire string for this status.
    pub fn as_str(&self) -> &'static str {
        match self {
            AliasStatus::Active => "active",
            AliasStatus::Revoked => "revoked",
        }
    }

    /// Parse the stored string form, failing closed on an unknown value.
    pub fn from_str_strict(s: &str) -> Result<Self> {
        match s {
            "active" => Ok(AliasStatus::Active),
            "revoked" => Ok(AliasStatus::Revoked),
            other => Err(CoreError::Alias(format!("unknown alias status {other:?}"))),
        }
    }
}

/// A persisted email-alias record (C3 #17).
///
/// Maps one address to the `(persona, site)` pair it fronts. Persisted in the
/// `email_aliases` table; the [`crate::store::EncryptedStore`] round-trips this
/// whole record. The address itself is the only secret-ish value, and it lives
/// in the encrypted DB with the rest of the mapping; PROVIDER credentials (the
/// API token a future HTTP provider would use) do NOT live here, they go in the
/// OS keystore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAlias {
    /// Stable id for this alias (UUID v4 string).
    pub id: String,
    /// The persona this alias belongs to.
    pub persona_id: String,
    /// The site this alias fronts (a normalized host/domain or a label).
    pub site: String,
    /// The email address itself (`base+tag@domain` or a masked forward).
    pub address: String,
    /// How the address was produced.
    pub kind: AliasKind,
    /// Current lifecycle status.
    pub status: AliasStatus,
    /// Epoch millis the alias was created.
    pub created_at: i64,
    /// Optional provider id this alias came from (e.g. a future "addy" /
    /// "simplelogin"), or `None` for a local plus-address. Just a label; the
    /// provider's secret lives in the OS keystore, never here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl EmailAlias {
    /// Build a new active alias record.
    pub fn new(
        id: String,
        persona_id: &str,
        site: &str,
        address: &str,
        kind: AliasKind,
        created_at: i64,
        provider: Option<String>,
    ) -> Self {
        Self {
            id,
            persona_id: persona_id.to_string(),
            site: site.to_string(),
            address: address.to_string(),
            kind,
            status: AliasStatus::Active,
            created_at,
            provider,
        }
    }

    /// Whether this alias is currently active.
    pub fn is_active(&self) -> bool {
        self.status == AliasStatus::Active
    }
}

/// The seam a masked-alias PROVIDER plugs into (C3 #17).
///
/// Object-safe and async so a future HTTP-backed provider (addy.io,
/// SimpleLogin, Apple Hide-My-Email, ...) can mint a fresh forwarding address
/// for a `(persona, site)` pair behind this trait. The HTTP integration is
/// DEFERRED (no direct `reqwest` dependency is declared), so the only built-in
/// impl is the local, network-free [`PlusAddressProvider`]. A real provider's
/// credentials are loaded from the OS keystore by its constructor and never
/// passed through this trait or stored in the DB.
#[async_trait]
pub trait AliasProvider: Send + Sync {
    /// A stable id for this provider, recorded on the minted alias (e.g.
    /// `"plus"`, or a future `"addy"`).
    fn id(&self) -> &str;

    /// The kind of address this provider mints.
    fn kind(&self) -> AliasKind;

    /// Mint a fresh address for `(persona_id, site)`. The local plus-address
    /// provider derives it deterministically; a future HTTP provider performs a
    /// network call here (and is the reason this is async).
    async fn mint(&self, persona_id: &str, site: &str) -> Result<String>;
}

/// The local, network-free alias provider: generates plus-addresses
/// (`base+tag@domain`) from a configured base address. This is the only
/// built-in provider until the HTTP masking-provider integration lands.
///
/// The tag is derived from the site (and, when distinctness across personas on
/// the same base is wanted, a short persona discriminator), so the generated
/// address is stable and self-documenting. No network, no secret.
#[derive(Debug, Clone)]
pub struct PlusAddressProvider {
    local_part: String,
    domain: String,
    /// When true, the persona id is folded into the tag so two personas on the
    /// same base address fronting the same site still get distinct tags.
    persona_in_tag: bool,
}

impl PlusAddressProvider {
    /// Build a provider over the base address `local_part@domain`. Fails closed
    /// if the base is not a single `local@domain` with non-empty halves and no
    /// existing `+` tag.
    pub fn new(base_address: &str) -> Result<Self> {
        let (local_part, domain) = split_base_address(base_address)?;
        Ok(Self {
            local_part,
            domain,
            persona_in_tag: false,
        })
    }

    /// Fold the persona id into the tag (builder style) so two personas sharing
    /// the same base address get distinct addresses for the same site.
    pub fn with_persona_in_tag(mut self, on: bool) -> Self {
        self.persona_in_tag = on;
        self
    }

    /// Generate the plus-address for `(persona_id, site)` without async (the
    /// derivation is pure and local). [`AliasProvider::mint`] delegates here.
    pub fn generate(&self, persona_id: &str, site: &str) -> String {
        let site_tag = sanitize_tag(site);
        let tag = if self.persona_in_tag {
            let persona_tag = persona_discriminator(persona_id);
            format!("{site_tag}.{persona_tag}")
        } else {
            site_tag
        };
        format!("{}+{}@{}", self.local_part, tag, self.domain)
    }
}

#[async_trait]
impl AliasProvider for PlusAddressProvider {
    fn id(&self) -> &str {
        "plus"
    }

    fn kind(&self) -> AliasKind {
        AliasKind::PlusAddress
    }

    async fn mint(&self, persona_id: &str, site: &str) -> Result<String> {
        Ok(self.generate(persona_id, site))
    }
}

/// Split a base address into `(local_part, domain)`, failing closed on a
/// malformed base. The base must NOT already carry a `+` tag (we add one), and
/// both halves must be non-empty with exactly one `@`.
fn split_base_address(base: &str) -> Result<(String, String)> {
    let base = base.trim();
    let mut parts = base.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if local.is_empty() || domain.is_empty() || parts.next().is_some() {
        return Err(CoreError::Alias(format!(
            "base address {base:?} must be a single local@domain"
        )));
    }
    if local.contains('+') {
        return Err(CoreError::Alias(format!(
            "base address {base:?} must not already carry a + tag"
        )));
    }
    Ok((local.to_string(), domain.to_string()))
}

/// Turn a site identifier into a safe plus-tag: lowercase, keep alphanumerics
/// and dots/hyphens, collapse anything else to a hyphen, strip a leading
/// `www.` and any scheme. Stable for the same input.
fn sanitize_tag(site: &str) -> String {
    let mut s = site.trim().to_ascii_lowercase();
    // Drop a scheme if the caller passed a URL.
    if let Some(idx) = s.find("://") {
        s = s[idx + 3..].to_string();
    }
    // Keep only the host portion (before the first slash).
    if let Some(idx) = s.find('/') {
        s = s[..idx].to_string();
    }
    if let Some(stripped) = s.strip_prefix("www.") {
        s = stripped.to_string();
    }
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "site".to_string()
    } else {
        trimmed
    }
}

/// A short, stable discriminator for a persona id, used when folding the
/// persona into a plus-tag. Takes the leading alphanumerics of the id so it is
/// deterministic and email-safe.
fn persona_discriminator(persona_id: &str) -> String {
    let cleaned: String = persona_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    if cleaned.is_empty() {
        "p".to_string()
    } else {
        cleaned.to_ascii_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_status_strings_round_trip() -> Result<()> {
        for k in [AliasKind::PlusAddress, AliasKind::Masked] {
            assert_eq!(AliasKind::from_str_strict(k.as_str())?, k);
        }
        for s in [AliasStatus::Active, AliasStatus::Revoked] {
            assert_eq!(AliasStatus::from_str_strict(s.as_str())?, s);
        }
        assert!(matches!(
            AliasKind::from_str_strict("nope"),
            Err(CoreError::Alias(_))
        ));
        assert!(matches!(
            AliasStatus::from_str_strict("nope"),
            Err(CoreError::Alias(_))
        ));
        Ok(())
    }

    #[test]
    fn plus_address_generation_is_stable_and_well_formed() -> Result<()> {
        let p = PlusAddressProvider::new("alice@example.com")?;
        let a = p.generate("persona-1", "spokeo.com");
        assert_eq!(a, "alice+spokeo.com@example.com");
        // Stable for the same input.
        assert_eq!(a, p.generate("persona-1", "spokeo.com"));
        // Different sites yield different addresses.
        assert_ne!(a, p.generate("persona-1", "whitepages.com"));
        Ok(())
    }

    #[test]
    fn plus_address_sanitizes_urls_and_www() -> Result<()> {
        let p = PlusAddressProvider::new("bob@mail.test")?;
        assert_eq!(
            p.generate("p", "https://www.Example.com/optout"),
            "bob+example.com@mail.test"
        );
        // Odd characters collapse to hyphens, trimmed at the ends.
        assert_eq!(p.generate("p", "a b!c"), "bob+a-b-c@mail.test");
        Ok(())
    }

    #[test]
    fn persona_in_tag_distinguishes_personas_on_same_base() -> Result<()> {
        let p = PlusAddressProvider::new("alice@example.com")?.with_persona_in_tag(true);
        let a1 = p.generate("11111111-aaaa", "spokeo.com");
        let a2 = p.generate("22222222-bbbb", "spokeo.com");
        assert_ne!(a1, a2);
        assert!(a1.starts_with("alice+spokeo.com."));
        Ok(())
    }

    #[test]
    fn malformed_base_addresses_fail_closed() {
        for bad in [
            "no-at-sign",
            "@example.com",
            "alice@",
            "a@b@c",
            "alice+tag@example.com",
        ] {
            assert!(
                matches!(PlusAddressProvider::new(bad), Err(CoreError::Alias(_))),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[tokio::test]
    async fn provider_trait_mints_via_local_impl() -> Result<()> {
        let p = PlusAddressProvider::new("alice@example.com")?;
        assert_eq!(p.id(), "plus");
        assert_eq!(p.kind(), AliasKind::PlusAddress);
        let minted = p.mint("persona-1", "spokeo.com").await?;
        assert_eq!(minted, "alice+spokeo.com@example.com");
        // The trait object is usable behind a Box (the future-provider seam).
        let boxed: Box<dyn AliasProvider> = Box::new(p);
        assert_eq!(boxed.mint("p", "x.com").await?, "alice+x.com@example.com");
        Ok(())
    }
}
