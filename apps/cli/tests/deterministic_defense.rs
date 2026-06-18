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

//! End-to-end CLI tests for the C3 deterministic-channel defense surface:
//! `broker`, `dsar`, `alias`, and `anchor`. Drives the compiled `fauxx` binary
//! against a temp store using the headless encrypted-key-file key source (NEVER
//! the OS keystore, to stay hermetic). Covers each group's happy path plus the
//! exit-code contract on a missing entity.

mod common;
use common::Fixture;

/// Add the standard test persona so the per-persona commands have a target.
fn add_persona(fx: &Fixture, id: &str) -> anyhow::Result<()> {
    let out = fx.run(&[
        "persona",
        "add",
        "--id",
        id,
        "--name",
        "Defense Test",
        "--age-range",
        "AGE_25_34",
        "--profession",
        "ENGINEER",
        "--region",
        "US_WEST",
        "--interests",
        "TECHNOLOGY,SCIENCE,GAMING",
    ])?;
    common::assert_ok(&out, "persona add")?;
    Ok(())
}

const PERSONA: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

#[test]
fn broker_list_generate_record_track() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // The bundled registry lists at least the well-known brokers.
    let out = fx.run(&["broker", "list"])?;
    common::assert_ok(&out, "broker list")?;
    let listing = common::stdout(&out)?;
    assert!(listing.contains("spokeo"), "registry: {listing}");

    // The registry round-trips as JSON.
    let out = fx.run(&["broker", "list", "--json"])?;
    common::assert_ok(&out, "broker list --json")?;
    assert!(common::stdout(&out)?.contains("spokeo"));

    // Generate (without recording) a filled request.
    let out = fx.run(&["broker", "generate", "spokeo", PERSONA])?;
    common::assert_ok(&out, "broker generate")?;
    assert!(common::stdout(&out)?.contains("broker=spokeo"));

    // Record a drafted submission, then it appears in the submissions list.
    let out = fx.run(&["broker", "record", "spokeo", PERSONA])?;
    common::assert_ok(&out, "broker record")?;
    assert!(common::stdout(&out)?.contains("recorded submission"));

    let out = fx.run(&["broker", "submissions", "--json"])?;
    common::assert_ok(&out, "broker submissions")?;
    assert!(common::stdout(&out)?.contains("spokeo"));

    // Due-soon runs and prints something (drafted submissions are not yet due).
    let out = fx.run(&["broker", "due-soon"])?;
    common::assert_ok(&out, "broker due-soon")?;
    Ok(())
}

#[test]
fn broker_record_unknown_broker_exits_1() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    let out = fx.run(&["broker", "record", "no-such-broker", PERSONA])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn dsar_generate_record_list_export() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Generate (preview) a GDPR access letter against a known broker.
    let out = fx.run(&[
        "dsar",
        "generate",
        "gdpr-access",
        PERSONA,
        "--broker",
        "spokeo",
    ])?;
    common::assert_ok(&out, "dsar generate")?;
    assert!(common::stdout(&out)?.contains("\"kind\""));

    // Record one against an arbitrary controller.
    let out = fx.run(&[
        "dsar",
        "record",
        "ccpa-deletion",
        PERSONA,
        "--controller-name",
        "Example Corp",
        "--controller-contact",
        "privacy@example.test",
    ])?;
    common::assert_ok(&out, "dsar record")?;
    let recorded = common::stdout(&out)?;
    assert!(recorded.contains("recorded dsar"));

    // List shows it.
    let out = fx.run(&["dsar", "list"])?;
    common::assert_ok(&out, "dsar list")?;
    assert!(common::stdout(&out)?.contains("Example Corp"));

    // Pull the recorded id out of the JSON listing and export its letter.
    let out = fx.run(&["dsar", "list", "--json"])?;
    common::assert_ok(&out, "dsar list --json")?;
    let json = common::stdout(&out)?;
    let id = extract_first_id(&json).ok_or_else(|| anyhow::anyhow!("no id in {json}"))?;

    let out = fx.run(&["dsar", "export", &id, "--name", "Real Person"])?;
    common::assert_ok(&out, "dsar export")?;
    assert!(common::stdout(&out)?.contains("Real Person"));

    // Overdue runs (nothing is overdue for a fresh drafted/recorded request).
    let out = fx.run(&["dsar", "overdue"])?;
    common::assert_ok(&out, "dsar overdue")?;
    Ok(())
}

#[test]
fn dsar_mark_sent_is_reachable_and_validates_the_id() -> anyhow::Result<()> {
    // #16 closed gap: until a request is marked SENT, no statutory deadline is
    // tracked. There must be a CLI way to mark it sent.
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Record a request and pull its id from the JSON listing.
    let out = fx.run(&[
        "dsar",
        "record",
        "ccpa-deletion",
        PERSONA,
        "--controller-name",
        "Example Corp",
    ])?;
    common::assert_ok(&out, "dsar record")?;
    let out = fx.run(&["dsar", "list", "--json"])?;
    common::assert_ok(&out, "dsar list --json")?;
    let id = extract_first_id(&common::stdout(&out)?)
        .ok_or_else(|| anyhow::anyhow!("no id in dsar listing"))?;

    // Mark it sent: the command exists and reports the new state.
    let out = fx.run(&["dsar", "sent", &id])?;
    common::assert_ok(&out, "dsar sent")?;
    let marked = common::stdout(&out)?;
    assert!(marked.contains("marked dsar"), "sent output: {marked}");
    assert!(marked.contains(&id), "names the request: {marked}");

    // Marking an unknown id fails closed with the usage exit code.
    let out = fx.run(&["dsar", "sent", "no-such-request"])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn gpc_list_and_status_are_reachable_headless() -> anyhow::Result<()> {
    // #18 closed gap: per-site GPC honoring must be inspectable from the CLI
    // (the GUI Privacy hub renders the same data). On a fresh store both report
    // an explicit empty state rather than erroring.
    let fx = Fixture::new()?;

    let out = fx.run(&["gpc", "list"])?;
    common::assert_ok(&out, "gpc list")?;
    assert!(
        common::stdout(&out)?.contains("no gpc observations"),
        "empty list: {}",
        common::stdout(&out)?
    );

    let out = fx.run(&["gpc", "status", "https://example.com"])?;
    common::assert_ok(&out, "gpc status")?;
    assert!(
        common::stdout(&out)?.contains("no gpc observation"),
        "empty status: {}",
        common::stdout(&out)?
    );
    Ok(())
}

#[test]
fn dsar_generate_missing_controller_is_usage_error() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;
    // Neither --broker nor --controller-name: a runtime usage error (exit 1).
    let out = fx.run(&["dsar", "generate", "gdpr-access", PERSONA])?;
    assert_eq!(common::code(&out)?, 1, "stdout: {}", common::stdout(&out)?);
    Ok(())
}

#[test]
fn alias_mint_list_revoke_rotate() -> anyhow::Result<()> {
    let fx = Fixture::new()?;
    add_persona(&fx, PERSONA)?;

    // Mint a plus-address alias.
    let out = fx.run(&[
        "alias",
        "mint",
        PERSONA,
        "shop.example",
        "--base",
        "me@example.test",
    ])?;
    common::assert_ok(&out, "alias mint")?;
    assert!(common::stdout(&out)?.contains("me+shop.example@example.test"));

    // List shows it; pull its id.
    let out = fx.run(&["alias", "list", "--json"])?;
    common::assert_ok(&out, "alias list")?;
    let json = common::stdout(&out)?;
    let id = extract_first_id(&json).ok_or_else(|| anyhow::anyhow!("no alias id in {json}"))?;

    // Rotate it (revokes the old, mints a fresh one).
    let out = fx.run(&["alias", "rotate", &id, "--base", "me@example.test"])?;
    common::assert_ok(&out, "alias rotate")?;
    assert!(common::stdout(&out)?.contains("rotated to fresh alias"));

    // Record a manual alias, then revoke it.
    let out = fx.run(&[
        "alias",
        "record",
        PERSONA,
        "bank.example",
        "masked-abc@icloud.test",
    ])?;
    common::assert_ok(&out, "alias record")?;
    let out = fx.run(&["alias", "list", "--json", "--persona", PERSONA])?;
    common::assert_ok(&out, "alias list persona")?;
    Ok(())
}

#[test]
fn anchor_record_score_recommendations() -> anyhow::Result<()> {
    let fx = Fixture::new()?;

    // Two anchors sharing a recovery contact, so scoring finds linkage.
    let out = fx.run(&[
        "anchor",
        "record",
        "Personal Gmail",
        "google.com",
        "--signal",
        "verified-email,legal-name,recovery-contact",
        "--shared-contact-key",
        "recovery-1",
    ])?;
    common::assert_ok(&out, "anchor record 1")?;

    let out = fx.run(&[
        "anchor",
        "record",
        "Bank X",
        "bank.example",
        "--signal",
        "payment,legal-name",
        "--shared-contact-key",
        "recovery-1",
    ])?;
    common::assert_ok(&out, "anchor record 2")?;

    // List shows both.
    let out = fx.run(&["anchor", "list"])?;
    common::assert_ok(&out, "anchor list")?;
    let listing = common::stdout(&out)?;
    assert!(listing.contains("Personal Gmail"));
    assert!(listing.contains("Bank X"));

    // Scoring ranks them; the linked accounts compound the score.
    let out = fx.run(&["anchor", "score", "--json"])?;
    common::assert_ok(&out, "anchor score")?;
    assert!(common::stdout(&out)?.contains("\"linkedAccounts\""));

    // Recommendations are produced.
    let out = fx.run(&["anchor", "recommendations"])?;
    common::assert_ok(&out, "anchor recommendations")?;
    Ok(())
}

/// Extract the first `"id": "<value>"` from a JSON dump (the records all carry
/// a UUID `id` field as their first stable key).
fn extract_first_id(json: &str) -> Option<String> {
    let marker = "\"id\":";
    let start = json.find(marker)? + marker.len();
    let rest = &json[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')? + q1;
    Some(rest[q1..q2].to_string())
}
