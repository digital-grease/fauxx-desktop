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

//! `fauxx native-host`: the R4 native-messaging HOST that bridges the C2 #14
//! WebExtension to the headless [`fauxx_core::Core`] (C2 #11-#13).
//!
//! The browser launches this process and exchanges JSON objects with it over
//! stdin/stdout, each framed by a 4-byte NATIVE-ENDIAN `u32` length prefix (the
//! Chromium / Firefox native-messaging transport). This module owns:
//!
//! 1. the framing [`codec`] (`read_message`/`write_message`) with an oversized
//!    frame guard, hermetic-testable over any [`std::io::Read`]/[`Write`] (e.g.
//!    an in-memory `Cursor`), and
//! 2. the request [`dispatch`]: it serves decoy plans (host -> extension) built
//!    from the persona's interests via the core's category-targeting API and
//!    persists the extension's reported activity / Topics read-backs / GPC
//!    observations into the measurement store (extension -> host).
//!
//! The wire schema is the contract in `extension/PROTOCOL.md` and
//! `extension/src/protocol.js`. Like the native decoy path, the host enforces
//! the SAME hard guardrails (decoy-only intent, HTTPS-only targets, the
//! `AUTH_FLOW_BLOCKLIST`); the extension re-checks every target too (defense in
//! depth). Nothing leaves the machine except the decoy traffic the extension
//! itself issues; this host is purely local stdio.

use std::io::{self, Read, Write};

use anyhow::Context as _;
use fauxx_core::browser::isolation;
use fauxx_core::persona::CategoryPool;
use fauxx_core::{Config, Core, GpcSupport, IntensityLevel, SyntheticPersona};
use serde::{Deserialize, Serialize};

/// The protocol schema version both sides speak (`PROTOCOL_VERSION` in
/// `extension/src/protocol.js`). A message with a different `v` is refused.
pub const PROTOCOL_VERSION: u32 = 1;

/// The only acceptable decoy-plan intent: this path is decoy-only by
/// construction (`REQUIRED_INTENT` in `extension/src/protocol.js`).
pub const REQUIRED_INTENT: &str = "decoy";

/// The default cap on sites a single decoy plan touches when the budget is not
/// otherwise constrained. Mirrors the extension's `maxTargets` default of 12.
pub const DEFAULT_MAX_TARGETS: usize = 12;

/// Run the `native-host` subcommand: open the core, then bridge the browser's
/// native-messaging stdio to the [`Core`] API until EOF (the browser closed).
///
/// This is the production entrypoint. It locks real stdin/stdout and drives the
/// shared [`serve`] loop over them; [`serve`] itself is generic over the
/// reader/writer so the hermetic tests drive it over in-memory `Cursor`s with no
/// real stdio and no browser.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let core = Core::open(config)
        .await
        .context("opening the core for the native-messaging host")?;

    // Lock real stdin/stdout once for the lifetime of the session. The browser
    // owns the other end of these pipes.
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    serve(&core, &mut reader, &mut writer).await
}

/// Drive the native-messaging session to completion over arbitrary byte
/// streams.
///
/// Sends the `hello` handshake, then loops: read one framed request, dispatch
/// it against the core, write any framed response, and flush. Returns `Ok(())`
/// on a clean EOF (the browser closed its end). A malformed/oversized frame is a
/// hard error that ends the session (fail closed). Generic over the reader and
/// writer so the tests exercise the exact same path over a `Cursor`.
pub async fn serve<R: Read, W: Write>(
    core: &Core,
    reader: &mut R,
    writer: &mut W,
) -> anyhow::Result<()> {
    // Handshake: announce the core version and the negotiated schema version.
    let hello = HostMessage::Hello {
        v: PROTOCOL_VERSION,
        core_version: core.version().to_string(),
        schema_version: PROTOCOL_VERSION,
    };
    codec::write_message(writer, &hello).context("writing the hello handshake")?;
    writer.flush().context("flushing the hello handshake")?;

    loop {
        // A clean EOF (browser closed) ends the loop with success.
        let Some(request) = codec::read_message::<_, ExtMessage>(reader)
            .context("reading a native-messaging request frame")?
        else {
            tracing::info!(
                target: "fauxx_cli::native_host",
                "native-messaging stdin closed (browser disconnected); exiting cleanly"
            );
            return Ok(());
        };

        // Dispatch produces zero or more reply frames (e.g. a `decoyPlan` for a
        // request, or nothing for a fire-and-forget report).
        let replies = dispatch::handle(core, request).await;
        for reply in replies {
            codec::write_message(writer, &reply).context("writing a native-messaging reply")?;
        }
        writer
            .flush()
            .context("flushing native-messaging replies")?;
    }
}

/// Frame codec for the browser native-messaging transport.
///
/// Each message on the wire is a 4-byte NATIVE-ENDIAN `u32` length prefix
/// followed by exactly that many bytes of UTF-8 JSON. Chromium and Firefox both
/// use the host platform's native byte order for the prefix (not network order),
/// so we encode/decode with [`u32::to_ne_bytes`] / [`u32::from_ne_bytes`].
///
/// A hard [`MAX_MESSAGE_LEN`] guard rejects an oversized frame BEFORE allocating
/// a buffer for it, so a hostile or buggy peer cannot drive an unbounded
/// allocation; the codec fails closed instead.
pub mod codec {
    use super::*;

    /// Maximum accepted frame body length, in bytes. The native-messaging spec
    /// caps a message the browser will RECEIVE from a host at 1 MiB; we cap what
    /// we will SEND or accept at the same bound so an oversized frame fails
    /// closed rather than allocating an unbounded buffer.
    pub const MAX_MESSAGE_LEN: u32 = 1024 * 1024;

    /// Serialize `message` to JSON and write it as one native-messaging frame:
    /// the native-endian `u32` length prefix followed by the JSON body.
    ///
    /// Fails closed (and writes NOTHING) if the encoded body exceeds
    /// [`MAX_MESSAGE_LEN`], so the host never emits a frame the browser would
    /// reject anyway.
    pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
        let body = serde_json::to_vec(message)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let len = u32::try_from(body.len())
            .ok()
            .filter(|n| *n <= MAX_MESSAGE_LEN);
        let Some(len) = len else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "refusing to write oversized native-messaging frame: {} bytes (max {})",
                    body.len(),
                    MAX_MESSAGE_LEN
                ),
            ));
        };
        // Native-endian length prefix, then the JSON body. One write each; the
        // caller flushes.
        writer.write_all(&len.to_ne_bytes())?;
        writer.write_all(&body)?;
        Ok(())
    }

    /// Read one native-messaging frame and deserialize its JSON body into `T`.
    ///
    /// Returns `Ok(None)` on a clean EOF at a frame boundary (no length prefix
    /// available: the peer closed its end). Returns an error on a TRUNCATED frame
    /// (EOF mid-prefix or mid-body), on an OVERSIZED length prefix (> [`MAX_MESSAGE_LEN`],
    /// rejected before allocating the body buffer), or on a body that is not the
    /// expected UTF-8 JSON. Fail closed: a frame we cannot fully and safely read
    /// is an error, never a partial value.
    pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(
        reader: &mut R,
    ) -> io::Result<Option<T>> {
        // Read the 4-byte native-endian length prefix. A clean EOF here (zero
        // bytes) is the normal end of the session.
        let mut len_buf = [0u8; 4];
        match read_exact_or_eof(reader, &mut len_buf)? {
            ReadExact::Eof => return Ok(None),
            ReadExact::Truncated(n) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("truncated native-messaging length prefix: got {n} of 4 bytes"),
                ));
            }
            ReadExact::Filled => {}
        }
        let len = u32::from_ne_bytes(len_buf);

        // Guard BEFORE allocating: an oversized prefix fails closed and never
        // reserves a giant buffer.
        if len > MAX_MESSAGE_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("oversized native-messaging frame: {len} bytes (max {MAX_MESSAGE_LEN})"),
            ));
        }

        // Read exactly `len` body bytes. A short read here is a TRUNCATED frame
        // (the peer promised `len` bytes then closed), which is an error.
        let mut body = vec![0u8; len as usize];
        match read_exact_or_eof(reader, &mut body)? {
            ReadExact::Filled => {}
            ReadExact::Eof | ReadExact::Truncated(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("truncated native-messaging frame body (expected {len} bytes)"),
                ));
            }
        }

        let value = serde_json::from_slice::<T>(&body)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(value))
    }

    /// Outcome of [`read_exact_or_eof`]: the buffer was fully filled, the stream
    /// was at EOF before any byte was read, or it ended partway through.
    enum ReadExact {
        /// The whole buffer was filled.
        Filled,
        /// EOF at a clean boundary: zero bytes read into the buffer.
        Eof,
        /// EOF after `usize` bytes (more than zero, fewer than the buffer).
        Truncated(usize),
    }

    /// Fill `buf` completely, distinguishing a clean boundary EOF (nothing read)
    /// from a truncation (some bytes read, then EOF). Unlike [`Read::read_exact`],
    /// a zero-length first read is reported as [`ReadExact::Eof`] rather than an
    /// error, which is how the loop detects the browser closing cleanly.
    fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<ReadExact> {
        let mut filled = 0;
        while filled < buf.len() {
            match reader.read(&mut buf[filled..]) {
                Ok(0) => {
                    return Ok(if filled == 0 {
                        ReadExact::Eof
                    } else {
                        ReadExact::Truncated(filled)
                    });
                }
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(ReadExact::Filled)
    }
}

/// Request dispatch: map one inbound [`ExtMessage`] to the core actions it
/// triggers and the [`HostMessage`] replies to send back.
pub mod dispatch {
    use super::*;

    /// Handle one extension -> host message, returning the host -> extension
    /// replies to write (often empty: most reports are fire-and-forget).
    ///
    /// A core error while persisting a report is surfaced as an `error` reply to
    /// the extension rather than aborting the whole session: one bad report must
    /// not tear down the bridge.
    pub async fn handle(core: &Core, message: ExtMessage) -> Vec<HostMessage> {
        match message {
            // Handshake reply: nothing to send back, just log it locally. A
            // schema-version mismatch is logged (the extension also refuses a
            // mismatched `v`); the bridge stays up so the operator sees it.
            ExtMessage::Ready {
                extension_version,
                schema_version,
            } => {
                if schema_version != PROTOCOL_VERSION {
                    tracing::warn!(
                        target: "fauxx_cli::native_host",
                        %extension_version,
                        extension_schema = schema_version,
                        host_schema = PROTOCOL_VERSION,
                        "extension schema version differs from the host"
                    );
                } else {
                    tracing::info!(
                        target: "fauxx_cli::native_host",
                        %extension_version,
                        schema_version,
                        "extension ready"
                    );
                }
                Vec::new()
            }

            // The extension asks for a decoy plan for a persona; build one biased
            // by that persona's interests and hand it back.
            ExtMessage::RequestPlan {
                persona_id,
                intensity,
                seed,
                max_targets,
            } => match build_decoy_plan(
                core,
                persona_id.as_deref(),
                intensity.as_deref(),
                seed.unwrap_or(0),
                max_targets,
            )
            .await
            {
                Ok(plan) => vec![plan],
                Err(e) => vec![HostMessage::error("requestPlan", &e.to_string())],
            },

            // Activity report for a completed plan: persist the in-browser decoy
            // session into the measurement store.
            ExtMessage::DecoyReport(report) => match record_decoy_report(core, &report).await {
                Ok(()) => Vec::new(),
                Err(e) => vec![HostMessage::error("decoyReport", &e.to_string())],
            },

            // A Privacy Sandbox Topics read from a decoy tab. The raw payload uses
            // the protocol's `topic` key, so run it through the core's lenient
            // parser (which owns that field naming) before persisting.
            ExtMessage::TopicsReadback {
                persona_id,
                decoy_id,
                readback,
            } => match fauxx_core::browser::topics::parse_topics_payload(&readback) {
                Ok(parsed) => match core
                    .record_topics_readback(&persona_id, &decoy_id, &parsed)
                    .await
                {
                    Ok(_) => Vec::new(),
                    Err(e) => vec![HostMessage::error("topicsReadback", &e.to_string())],
                },
                Err(e) => vec![HostMessage::error("topicsReadback", &e.to_string())],
            },

            // A parsed /.well-known/gpc.json observation.
            ExtMessage::GpcStatus { origin, support } => {
                match core.record_gpc_status(&origin, support).await {
                    Ok(_) => Vec::new(),
                    Err(e) => vec![HostMessage::error("gpcStatus", &e.to_string())],
                }
            }

            // A non-fatal problem the extension surfaced; log it locally only.
            ExtMessage::Error { context, message } => {
                tracing::warn!(
                    target: "fauxx_cli::native_host",
                    %context,
                    %message,
                    "extension reported a non-fatal error"
                );
                Vec::new()
            }
        }
    }

    /// Build a decoy plan for `persona_id`, biased by the persona's interests.
    ///
    /// Resolves the persona's [`CategoryPool`] interests to the curated, bundled
    /// HTTPS site set (the core's category-targeting API), enforces the SAME hard
    /// guardrails as the native decoy path (HTTPS-only, no auth-flow hosts), caps
    /// the targets by the requested/default budget, and emits a strictly
    /// decoy-intent [`HostMessage::DecoyPlan`]. Errors if the persona is unknown
    /// or has no resolvable, eligible sites.
    /// Resolve the persona a plan is for: the one named by `persona_id`, or (when
    /// it is absent or empty) the first persona in the store as the default. This
    /// is what lets the extension pull a plan with no configuration in the common
    /// single-persona case; errors when the named persona is unknown or the store
    /// has no personas at all.
    async fn resolve_persona(
        core: &Core,
        persona_id: Option<&str>,
    ) -> anyhow::Result<SyntheticPersona> {
        match persona_id {
            Some(id) if !id.is_empty() => core
                .get_persona(id)
                .await
                .with_context(|| format!("loading persona {id} for a decoy plan")),
            _ => core
                .list_personas()
                .await
                .context("listing personas for a default decoy plan")?
                .into_iter()
                .next()
                .context("no personas in the store; cannot build a default decoy plan"),
        }
    }

    pub async fn build_decoy_plan(
        core: &Core,
        persona_id: Option<&str>,
        intensity: Option<&str>,
        seed: u64,
        max_targets: Option<usize>,
    ) -> anyhow::Result<HostMessage> {
        let persona = resolve_persona(core, persona_id).await?;

        let extension_intensity = parse_intensity(intensity);
        let base_categories = persona_categories(&persona);

        // Let a RUNNING campaign for this persona steer the plan (C8 #33 closed
        // loop): gap-to-goal overrides the intensity and biases topic selection
        // toward the target segment. With no running campaign the extension's
        // requested intensity and the persona's interest order stand (the biased
        // list equals the persona's categories when the directive is idle).
        let directive = core
            .campaign_directive_for_persona(&persona.id)
            .await
            .unwrap_or_else(|_| fauxx_core::CampaignDirective::idle());
        let intensity = directive.intensity.unwrap_or(extension_intensity);
        let categories = directive.bias_categories(&base_categories);
        let target_segment = categories.first().cloned().unwrap_or_default();

        // Resolve the (campaign-biased) categories to the bundled HTTPS sites in
        // that order, so the targeted segment's pages lead the capped plan, then
        // apply the guardrails (defense in depth: the core's table is already
        // HTTPS and off the blocklist, but we re-check so a future table edit
        // cannot leak a bad target through this path).
        let resolved = sites_for_biased_categories(&categories, &persona);
        let mut targets: Vec<String> = resolved
            .into_iter()
            .filter(|url| is_guardrail_safe(url))
            .collect();

        let cap = max_targets.unwrap_or(DEFAULT_MAX_TARGETS).max(1);
        targets.truncate(cap);

        if targets.is_empty() {
            anyhow::bail!(
                "persona {} resolves to no eligible decoy targets \
                 (no known interest categories with bundled HTTPS sites)",
                persona.id
            );
        }

        Ok(HostMessage::DecoyPlan(Box::new(DecoyPlan {
            v: PROTOCOL_VERSION,
            plan_id: uuid::Uuid::new_v4().to_string(),
            persona_id: persona.id.clone(),
            intent: REQUIRED_INTENT.to_string(),
            intensity: intensity_name(intensity).to_string(),
            target_segment,
            categories,
            targets,
            mode: "fetch".to_string(),
            gpc: true,
            max_targets: cap,
            seed,
        })))
    }

    /// Persist a reported in-browser decoy session into the measurement store.
    ///
    /// The `decoyReport` shape mirrors `fauxx_core::browser::SeedOutcome`
    /// exactly. We record the visited sites and the GPC honoring the decoy can
    /// observe for them through the same measurement plumbing the native path
    /// uses, attributing the activity to the persona. A guardrail re-check drops
    /// any visited URL that should never have been touched (it is logged, not
    /// recorded), so a misbehaving extension cannot launder a blocked target into
    /// the store.
    pub async fn record_decoy_report(core: &Core, report: &DecoyReport) -> anyhow::Result<()> {
        // Validate the persona exists before recording anything (fail closed on a
        // typo or a stale plan id).
        let _ = core
            .get_persona(&report.persona_id)
            .await
            .with_context(|| {
                format!(
                    "loading persona {} for a decoy activity report",
                    report.persona_id
                )
            })?;

        let mut recorded = 0usize;
        for url in &report.visited {
            if !is_guardrail_safe(url) {
                tracing::warn!(
                    target: "fauxx_cli::native_host",
                    persona_id = %report.persona_id,
                    blocked_url = %url,
                    "dropping a reported visit to a guardrail-blocked target"
                );
                continue;
            }
            // Persist the visit as a per-origin record via the measurement store.
            // We fold the in-browser session into the same GPC/site plumbing the
            // native SeedOutcome path feeds: each visited origin becomes a
            // "checked" site record attributed to the persona's decoy activity.
            let origin = origin_of(url);
            core.record_gpc_status(&origin, GpcSupport::not_advertised())
                .await
                .with_context(|| format!("recording decoy visit to {origin}"))?;
            recorded += 1;
        }

        // Each skipped target carries a local reason (a fetch failure or a
        // guardrail refusal); log them so the operator can see why a site was
        // not reached without anything leaving the machine.
        for skipped in &report.skipped {
            tracing::debug!(
                target: "fauxx_cli::native_host",
                persona_id = %report.persona_id,
                url = %skipped.url,
                reason = %skipped.reason,
                "reported decoy target skipped"
            );
        }

        tracing::info!(
            target: "fauxx_cli::native_host",
            persona_id = %report.persona_id,
            plan_id = %report.plan_id,
            seed = report.seed,
            visited = recorded,
            skipped = report.skipped.len(),
            started_at = report.started_at,
            finished_at = report.finished_at,
            "recorded in-browser decoy session"
        );
        Ok(())
    }

    /// The persona's interest names that map to a known [`CategoryPool`], in
    /// first-seen order (unknown legacy/future interests are skipped, matching
    /// the core's [`fauxx_core::browser::sites_for_persona`] behavior).
    fn persona_categories(persona: &SyntheticPersona) -> Vec<String> {
        persona
            .interests
            .iter()
            .filter(|i| CategoryPool::from_name(i).is_some())
            .cloned()
            .collect()
    }

    /// Resolve the (campaign-biased) category NAMES to decoy sites IN THAT ORDER
    /// via the core's category->sites table, so a campaign's targeted segment
    /// leads. Falls back to the persona's own resolution when no name maps to a
    /// known category (defensive: when no campaign runs the biased list equals the
    /// persona's categories, so this matches the prior `sites_for_persona` result).
    fn sites_for_biased_categories(
        categories: &[String],
        persona: &SyntheticPersona,
    ) -> Vec<String> {
        let pools: Vec<CategoryPool> = categories
            .iter()
            .filter_map(|name| CategoryPool::from_name(name))
            .collect();
        if pools.is_empty() {
            return fauxx_core::browser::sites_for_persona(persona);
        }
        fauxx_core::browser::sites_for_categories(&pools)
    }

    /// Whether a resolved target clears the SAME hard guardrails the native decoy
    /// path enforces: it must be an HTTPS URL and must not be an
    /// authenticated-account sign-in endpoint (the `AUTH_FLOW_BLOCKLIST`).
    fn is_guardrail_safe(url: &str) -> bool {
        url.starts_with("https://") && !isolation::is_blocked_auth_flow(url)
    }

    /// Reduce a URL to its `scheme://authority` origin for a per-site record.
    /// Falls back to the whole URL when it has no `://` separator.
    fn origin_of(url: &str) -> String {
        match url.split_once("://") {
            Some((scheme, rest)) => {
                let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                format!("{scheme}://{}", &rest[..auth_end])
            }
            None => url.to_string(),
        }
    }

    /// Parse an [`IntensityLevel`] name (the protocol uses the `IntensityLevel`
    /// variant names), defaulting to [`IntensityLevel::Medium`] when absent or
    /// unrecognized.
    fn parse_intensity(name: Option<&str>) -> IntensityLevel {
        match name {
            Some("Low") => IntensityLevel::Low,
            Some("Medium") => IntensityLevel::Medium,
            Some("High") => IntensityLevel::High,
            Some("Extreme") => IntensityLevel::Extreme,
            _ => IntensityLevel::Medium,
        }
    }

    /// The protocol name for an [`IntensityLevel`] (mirrors the variant name the
    /// extension's `INTENSITY_LEVELS` lists).
    fn intensity_name(level: IntensityLevel) -> &'static str {
        match level {
            IntensityLevel::Low => "Low",
            IntensityLevel::Medium => "Medium",
            IntensityLevel::High => "High",
            IntensityLevel::Extreme => "Extreme",
            // `IntensityLevel` is #[non_exhaustive]; default unknown future
            // variants to the middle of the ladder.
            _ => "Medium",
        }
    }
}

// ---------------------------------------------------------------------------
// Wire types (the contract in extension/PROTOCOL.md and extension/src/protocol.js)
// ---------------------------------------------------------------------------

/// A host -> extension message. The `type` discriminator and `v` schema version
/// are emitted on the wire (camelCase fields throughout to match the JS).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HostMessage {
    /// Handshake sent on connect.
    #[serde(rename = "hello", rename_all = "camelCase")]
    Hello {
        /// The schema version field common to every message (always
        /// [`PROTOCOL_VERSION`]).
        v: u32,
        /// The running core version string.
        core_version: String,
        /// The negotiated schema version (echoes [`PROTOCOL_VERSION`]).
        schema_version: u32,
    },
    /// The authoritative decoy plan for one tick (boxed: it is the large
    /// variant, so boxing keeps the enum small).
    #[serde(rename = "decoyPlan")]
    DecoyPlan(Box<DecoyPlan>),
    /// A non-fatal problem the host surfaces to the extension.
    #[serde(rename = "error")]
    Error {
        /// The schema version field.
        #[serde(rename = "v")]
        v: u32,
        /// A short context label for where the error arose.
        context: String,
        /// The human-readable message.
        message: String,
    },
}

impl HostMessage {
    /// Build an `error` reply for `context` with `message`.
    fn error(context: &str, message: &str) -> Self {
        HostMessage::Error {
            v: PROTOCOL_VERSION,
            context: context.to_string(),
            message: message.to_string(),
        }
    }
}

/// A `decoyPlan` payload (host -> extension). Decoy-only by construction:
/// [`intent`](Self::intent) is always [`REQUIRED_INTENT`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoyPlan {
    /// The schema version (always [`PROTOCOL_VERSION`]).
    pub v: u32,
    /// Opaque id the extension echoes back in its `decoyReport`.
    pub plan_id: String,
    /// Which synthetic persona this plan serves.
    pub persona_id: String,
    /// Always `"decoy"`; the extension refuses any other value.
    pub intent: String,
    /// The [`IntensityLevel`] name to run at.
    pub intensity: String,
    /// The primary `CategoryPool` name to bias toward (may be empty).
    pub target_segment: String,
    /// The `CategoryPool` names this plan resolves to sites for.
    pub categories: Vec<String>,
    /// Explicit HTTPS targets the extension visits/fetches.
    pub targets: Vec<String>,
    /// `"fetch"` (background GET, default) or `"visit"` (opt-in tab).
    pub mode: String,
    /// Whether GPC is emitted on this plan's traffic (default `true`).
    pub gpc: bool,
    /// Cap on sites touched this tick.
    pub max_targets: usize,
    /// Determinism hint, echoed back in the report.
    pub seed: u64,
}

/// An extension -> host message. The `type` discriminator selects the variant;
/// the common `v` schema version is read but not required to be present on every
/// variant by serde (we validate it where it matters).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ExtMessage {
    /// Handshake reply on connect.
    #[serde(rename = "ready", rename_all = "camelCase")]
    Ready {
        /// The extension's version string.
        #[serde(default)]
        extension_version: String,
        /// The schema version the extension speaks.
        #[serde(default)]
        schema_version: u32,
    },
    /// The extension asks the host for a decoy plan for a persona. This is the
    /// host-driven plan-pull the extension's loop uses; it is NOT in the JS
    /// constant list (the JS only validates host -> extension `decoyPlan`), but
    /// the host accepts it as the request that yields a `decoyPlan` reply.
    #[serde(rename = "requestPlan", rename_all = "camelCase")]
    RequestPlan {
        /// The persona id to plan for. Optional: when absent or empty the host
        /// plans for a DEFAULT persona (the first in the store), so the extension
        /// can pull a plan with zero configuration in the common single-persona
        /// case. A multi-persona operator sends an explicit id.
        #[serde(default)]
        persona_id: Option<String>,
        /// Optional [`IntensityLevel`] name (defaults to `Medium`).
        #[serde(default)]
        intensity: Option<String>,
        /// Optional determinism seed (defaults to 0).
        #[serde(default)]
        seed: Option<u64>,
        /// Optional cap on the number of targets (defaults to
        /// [`DEFAULT_MAX_TARGETS`]).
        #[serde(default)]
        max_targets: Option<usize>,
    },
    /// Activity report for a completed plan (mirrors `SeedOutcome`).
    #[serde(rename = "decoyReport")]
    DecoyReport(DecoyReport),
    /// A Privacy Sandbox Topics read from a decoy tab.
    #[serde(rename = "topicsReadback", rename_all = "camelCase")]
    TopicsReadback {
        /// The persona the read is attributed to.
        persona_id: String,
        /// The decoy profile id the read came from.
        decoy_id: String,
        /// The raw `{ available, topics: [{ topic, .. }] }` read-back payload, as
        /// the extension emits it (the protocol shape uses the `topic` key, NOT
        /// the core's serde `topic_id`). The host runs it through
        /// [`fauxx_core::parse_gpc_well_known`]'s Topics sibling
        /// (`parse_topics_payload`, reachable via the same accepting shape) so it
        /// maps onto [`fauxx_core::TopicsReadback`]; kept as a raw [`Value`] here
        /// so the lenient core parser owns the field naming.
        ///
        /// [`Value`]: serde_json::Value
        readback: serde_json::Value,
    },
    /// A parsed `/.well-known/gpc.json` observation.
    #[serde(rename = "gpcStatus")]
    GpcStatus {
        /// The site origin the observation is for.
        origin: String,
        /// The parsed support (maps onto [`fauxx_core::GpcSupport`]).
        support: GpcSupport,
    },
    /// A non-fatal problem the extension surfaces locally.
    #[serde(rename = "error")]
    Error {
        /// A short context label.
        #[serde(default)]
        context: String,
        /// The human-readable message.
        #[serde(default)]
        message: String,
    },
}

/// A `decoyReport` payload (extension -> host). Mirrors
/// `fauxx_core::browser::SeedOutcome`: the URLs that loaded and the URLs skipped
/// with a reason.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoyReport {
    /// The plan id this report answers.
    pub plan_id: String,
    /// The persona the activity is attributed to.
    pub persona_id: String,
    /// The determinism seed echoed from the plan.
    #[serde(default)]
    pub seed: u64,
    /// URLs that loaded / fetched successfully.
    #[serde(default)]
    pub visited: Vec<String>,
    /// URLs that were skipped, each with a short reason.
    #[serde(default)]
    pub skipped: Vec<SkippedTarget>,
    /// Epoch-millis the plan started (informational).
    #[serde(default)]
    pub started_at: i64,
    /// Epoch-millis the plan finished (informational).
    #[serde(default)]
    pub finished_at: i64,
}

/// One skipped target in a [`DecoyReport`]: the URL and the local reason it was
/// skipped (a fetch failure or a guardrail refusal).
#[derive(Debug, Clone, Deserialize)]
pub struct SkippedTarget {
    /// The URL that was skipped.
    pub url: String,
    /// The short local reason.
    #[serde(default)]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_round_trips_a_json_message() -> anyhow::Result<()> {
        // Encode a host message, then decode it back from the same bytes and
        // confirm the length prefix is correct and native-endian.
        let plan = DecoyPlan {
            v: PROTOCOL_VERSION,
            plan_id: "plan-1".to_string(),
            persona_id: "persona-x".to_string(),
            intent: REQUIRED_INTENT.to_string(),
            intensity: "Medium".to_string(),
            target_segment: "TECHNOLOGY".to_string(),
            categories: vec!["TECHNOLOGY".to_string()],
            targets: vec!["https://example.com/".to_string()],
            mode: "fetch".to_string(),
            gpc: true,
            max_targets: 6,
            seed: 42,
        };

        let mut buf = Vec::new();
        codec::write_message(&mut buf, &plan)?;

        // The first 4 bytes are the native-endian length of the JSON body.
        let body = serde_json::to_vec(&plan)?;
        let expected_len = u32::try_from(body.len())?;
        assert_eq!(&buf[..4], &expected_len.to_ne_bytes());
        assert_eq!(&buf[4..], body.as_slice());

        // It decodes back to an equal value over a Cursor.
        let mut reader = Cursor::new(buf);
        let back: DecoyPlan = match codec::read_message(&mut reader)? {
            Some(frame) => frame,
            None => anyhow::bail!("expected a decoded frame, got a clean EOF"),
        };
        assert_eq!(back.plan_id, plan.plan_id);
        assert_eq!(back.targets, plan.targets);
        assert_eq!(back.seed, plan.seed);
        Ok(())
    }

    #[test]
    fn clean_eof_at_a_boundary_yields_none() -> anyhow::Result<()> {
        // An empty stream (no length prefix) is a clean EOF, not an error.
        let mut reader = Cursor::new(Vec::<u8>::new());
        let frame: Option<DecoyPlan> = codec::read_message(&mut reader)?;
        assert!(frame.is_none());
        Ok(())
    }

    /// Assert that reading a `DecoyPlan` frame from `bytes` fails with the given
    /// io error kind (used by the oversized/truncated-frame cases).
    fn assert_read_err(bytes: Vec<u8>, expected: io::ErrorKind, what: &str) {
        let mut reader = Cursor::new(bytes);
        let result: io::Result<Option<DecoyPlan>> = codec::read_message(&mut reader);
        match result {
            Ok(_) => panic!("{what}: expected an error, got a successful read"),
            Err(e) => assert_eq!(e.kind(), expected, "{what}: wrong error kind"),
        }
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocating() {
        // A length prefix above the cap fails closed without trying to read a
        // gigantic body. We give it ONLY the prefix; if it tried to allocate and
        // read the body it would block/EOF, but the guard rejects it first.
        let huge = codec::MAX_MESSAGE_LEN + 1;
        let mut bytes = huge.to_ne_bytes().to_vec();
        // A few stray body bytes; far fewer than `huge`.
        bytes.extend_from_slice(b"{}");
        assert_read_err(bytes, io::ErrorKind::InvalidData, "oversized frame");
    }

    #[test]
    fn truncated_prefix_is_an_error() {
        // Only 2 of the 4 prefix bytes: a truncated frame, not a clean EOF.
        assert_read_err(
            vec![1u8, 0u8],
            io::ErrorKind::UnexpectedEof,
            "truncated prefix",
        );
    }

    #[test]
    fn truncated_body_is_an_error() {
        // A valid prefix claiming 16 bytes, but only 4 body bytes follow.
        let len: u32 = 16;
        let mut bytes = len.to_ne_bytes().to_vec();
        bytes.extend_from_slice(b"{abc");
        assert_read_err(bytes, io::ErrorKind::UnexpectedEof, "truncated body");
    }

    #[test]
    fn write_refuses_an_oversized_body() {
        // Build a message whose JSON body exceeds the cap, and confirm the writer
        // fails closed and emits nothing.
        let big = "x".repeat((codec::MAX_MESSAGE_LEN as usize) + 16);
        let plan = DecoyPlan {
            v: PROTOCOL_VERSION,
            plan_id: big,
            persona_id: "p".to_string(),
            intent: REQUIRED_INTENT.to_string(),
            intensity: "Low".to_string(),
            target_segment: String::new(),
            categories: vec![],
            targets: vec![],
            mode: "fetch".to_string(),
            gpc: true,
            max_targets: 1,
            seed: 0,
        };
        let mut buf = Vec::new();
        let result = codec::write_message(&mut buf, &plan);
        assert!(result.is_err());
        assert!(buf.is_empty(), "no partial frame should be written");
    }

    #[test]
    fn hello_serializes_with_type_and_schema_version() -> anyhow::Result<()> {
        let hello = HostMessage::Hello {
            v: PROTOCOL_VERSION,
            core_version: "0.1.0".to_string(),
            schema_version: PROTOCOL_VERSION,
        };
        let json = serde_json::to_value(&hello)?;
        assert_eq!(json["type"], "hello");
        assert_eq!(json["v"], PROTOCOL_VERSION);
        assert_eq!(json["coreVersion"], "0.1.0");
        assert_eq!(json["schemaVersion"], PROTOCOL_VERSION);
        Ok(())
    }

    #[test]
    fn ext_request_plan_parses_from_protocol_json() -> anyhow::Result<()> {
        let msg: ExtMessage = serde_json::from_value(serde_json::json!({
            "v": 1,
            "type": "requestPlan",
            "personaId": "persona-tech-traveler",
            "intensity": "Medium",
            "seed": 42,
            "maxTargets": 6
        }))?;
        match msg {
            ExtMessage::RequestPlan {
                persona_id,
                intensity,
                seed,
                max_targets,
            } => {
                assert_eq!(persona_id.as_deref(), Some("persona-tech-traveler"));
                assert_eq!(intensity.as_deref(), Some("Medium"));
                assert_eq!(seed, Some(42));
                assert_eq!(max_targets, Some(6));
            }
            other => anyhow::bail!("expected RequestPlan, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn ext_decoy_report_parses_seed_outcome_shape() -> anyhow::Result<()> {
        let msg: ExtMessage = serde_json::from_value(serde_json::json!({
            "v": 1,
            "type": "decoyReport",
            "planId": "plan-1",
            "personaId": "persona-x",
            "seed": 42,
            "visited": ["https://www.theverge.com/"],
            "skipped": [{ "url": "https://x.test/", "reason": "Failed to fetch" }],
            "startedAt": 1765600000000_i64,
            "finishedAt": 1765600032000_i64
        }))?;
        match msg {
            ExtMessage::DecoyReport(report) => {
                assert_eq!(report.plan_id, "plan-1");
                assert_eq!(
                    report.visited,
                    vec!["https://www.theverge.com/".to_string()]
                );
                assert_eq!(report.skipped.len(), 1);
                assert_eq!(report.skipped[0].reason, "Failed to fetch");
            }
            other => anyhow::bail!("expected DecoyReport, got {other:?}"),
        }
        Ok(())
    }

    // --- Hermetic dispatch tests over a temp EncryptedFile store + Cursors ---

    /// Open a [`Core`] backed by a fresh temp `EncryptedFile` store (NEVER the OS
    /// keystore), so the dispatch tests are hermetic. Returns the core and the
    /// owning [`tempfile::TempDir`] (kept alive for the store's lifetime).
    async fn hermetic_core() -> anyhow::Result<(Core, tempfile::TempDir)> {
        let dir = tempfile::tempdir()?;
        let config = Config::new()
            .with_path(dir.path().join("fauxx.db"))
            .with_key_source(fauxx_core::KeySource::EncryptedFile {
                path: dir.path().join("fauxx.db.key"),
                passphrase: "native-host-test-passphrase".to_string(),
            });
        let core = Core::open(config).await?;
        Ok((core, dir))
    }

    /// A known persona with TECHNOLOGY/TRAVEL interests, so the category-targeting
    /// API resolves it to bundled HTTPS sites.
    fn tech_traveler() -> SyntheticPersona {
        SyntheticPersona::new(
            "persona-tech-traveler".to_string(),
            "Tech Traveler".to_string(),
            "AGE_35_44".to_string(),
            "ENGINEER".to_string(),
            "US_WEST".to_string(),
            vec![
                "TECHNOLOGY".to_string(),
                "TRAVEL".to_string(),
                "SCIENCE".to_string(),
            ],
            1_700_000_000_000,
            1_700_600_000_000,
        )
    }

    #[tokio::test]
    async fn dispatch_serves_a_well_formed_decoy_plan_for_a_known_persona() -> anyhow::Result<()> {
        let (core, _dir) = hermetic_core().await?;
        let persona = tech_traveler();
        core.save_persona(&persona).await?;

        // A `requestPlan` for the known persona yields exactly one `decoyPlan`.
        let replies = dispatch::handle(
            &core,
            ExtMessage::RequestPlan {
                persona_id: Some(persona.id.clone()),
                intensity: Some("Medium".to_string()),
                seed: Some(7),
                max_targets: Some(4),
            },
        )
        .await;

        assert_eq!(replies.len(), 1, "exactly one reply expected");
        let plan = match &replies[0] {
            HostMessage::DecoyPlan(plan) => plan,
            other => anyhow::bail!("expected a decoyPlan reply, got {other:?}"),
        };

        // Decoy-only by construction, well-formed, and within the budget.
        assert_eq!(plan.v, PROTOCOL_VERSION);
        assert_eq!(plan.intent, REQUIRED_INTENT);
        assert_eq!(plan.persona_id, persona.id);
        assert_eq!(plan.intensity, "Medium");
        assert_eq!(plan.seed, 7);
        assert!(!plan.plan_id.is_empty());
        assert!(!plan.targets.is_empty(), "plan must carry targets");
        assert!(plan.targets.len() <= 4, "targets within the requested cap");
        assert_eq!(plan.max_targets, 4);
        assert!(plan.gpc, "GPC defaults ON");
        assert_eq!(plan.mode, "fetch");
        // Every target clears the same guardrails the native path enforces.
        for url in &plan.targets {
            assert!(url.starts_with("https://"), "HTTPS-only target: {url}");
            assert!(
                !isolation::is_blocked_auth_flow(url),
                "no auth-flow target: {url}"
            );
        }
        // The categories are the persona's known interests, in order.
        assert_eq!(plan.categories, vec!["TECHNOLOGY", "TRAVEL", "SCIENCE"]);
        assert_eq!(plan.target_segment, "TECHNOLOGY");
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_rejects_an_unknown_persona_with_an_error_reply() -> anyhow::Result<()> {
        let (core, _dir) = hermetic_core().await?;
        let replies = dispatch::handle(
            &core,
            ExtMessage::RequestPlan {
                persona_id: Some("no-such-persona".to_string()),
                intensity: None,
                seed: None,
                max_targets: None,
            },
        )
        .await;
        assert_eq!(replies.len(), 1);
        match &replies[0] {
            HostMessage::Error { context, .. } => assert_eq!(context, "requestPlan"),
            other => anyhow::bail!("expected an error reply, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_defaults_to_the_first_persona_when_none_is_given() -> anyhow::Result<()> {
        // A requestPlan with NO personaId (the extension's zero-config pull) plans
        // for the only persona in the store.
        let (core, _dir) = hermetic_core().await?;
        let persona = tech_traveler();
        core.save_persona(&persona).await?;

        let replies = dispatch::handle(
            &core,
            ExtMessage::RequestPlan {
                persona_id: None,
                intensity: None,
                seed: None,
                max_targets: Some(3),
            },
        )
        .await;

        assert_eq!(replies.len(), 1);
        match &replies[0] {
            HostMessage::DecoyPlan(plan) => {
                assert_eq!(plan.persona_id, persona.id);
                assert!(!plan.targets.is_empty());
            }
            other => anyhow::bail!("expected a decoyPlan for the default persona, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_errors_when_no_personas_exist_and_none_is_given() -> anyhow::Result<()> {
        // Zero-config pull against an empty store: a clear error, not a panic.
        let (core, _dir) = hermetic_core().await?;
        let replies = dispatch::handle(
            &core,
            ExtMessage::RequestPlan {
                persona_id: None,
                intensity: None,
                seed: None,
                max_targets: None,
            },
        )
        .await;
        assert_eq!(replies.len(), 1);
        match &replies[0] {
            HostMessage::Error { context, .. } => assert_eq!(context, "requestPlan"),
            other => anyhow::bail!("expected an error reply, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_persists_a_reported_activity_record() -> anyhow::Result<()> {
        let (core, _dir) = hermetic_core().await?;
        let persona = tech_traveler();
        core.save_persona(&persona).await?;

        // A reported in-browser decoy session: two visited HTTPS sites and one
        // skipped. The blocked auth-flow URL must be dropped, not persisted.
        let report = ExtMessage::DecoyReport(DecoyReport {
            plan_id: "plan-abc".to_string(),
            persona_id: persona.id.clone(),
            seed: 7,
            visited: vec![
                "https://www.theverge.com/".to_string(),
                "https://arstechnica.com/some/path".to_string(),
                // Must be dropped by the guardrail re-check.
                "https://accounts.google.com/signin".to_string(),
            ],
            skipped: vec![SkippedTarget {
                url: "https://tripadvisor.test/".to_string(),
                reason: "Failed to fetch".to_string(),
            }],
            started_at: 1_765_600_000_000,
            finished_at: 1_765_600_032_000,
        });

        let replies = dispatch::handle(&core, report).await;
        assert!(replies.is_empty(), "a report is fire-and-forget");

        // The two safe origins were persisted; the blocked one was not.
        assert!(core
            .gpc_status_for("https://www.theverge.com")
            .await?
            .is_some());
        assert!(core
            .gpc_status_for("https://arstechnica.com")
            .await?
            .is_some());
        assert!(core
            .gpc_status_for("https://accounts.google.com")
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_persists_a_topics_readback() -> anyhow::Result<()> {
        let (core, _dir) = hermetic_core().await?;
        let persona = tech_traveler();
        core.save_persona(&persona).await?;

        // The exact topicsReadback shape from PROTOCOL.md.
        let msg: ExtMessage = serde_json::from_value(serde_json::json!({
            "v": 1,
            "type": "topicsReadback",
            "personaId": persona.id,
            "decoyId": "ext-decoy-default",
            "readback": { "available": true, "topics": [{ "topic": 57, "taxonomyVersion": "1" }] }
        }))?;
        let replies = dispatch::handle(&core, msg).await;
        assert!(replies.is_empty());

        let latest = core.latest_topics_measurement(&persona.id).await?;
        let measurement = match latest {
            Some(m) => m,
            None => anyhow::bail!("expected a persisted topics measurement"),
        };
        assert!(measurement.available);
        assert_eq!(measurement.topics.len(), 1);
        assert_eq!(measurement.topics[0].topic_id, 57);
        Ok(())
    }

    #[tokio::test]
    async fn serve_handshakes_then_exits_cleanly_on_eof() -> anyhow::Result<()> {
        let (core, _dir) = hermetic_core().await?;
        let persona = tech_traveler();
        core.save_persona(&persona).await?;

        // Drive the WHOLE serve loop over in-memory Cursors (no real stdio): the
        // input is one `requestPlan` frame (framed exactly as the extension would
        // frame it) followed by EOF.
        let request_json = serde_json::json!({
            "v": 1,
            "type": "requestPlan",
            "personaId": persona.id,
            "intensity": "Low",
            "seed": 1,
            "maxTargets": 3
        });
        // Round-trip the request through the wire type to confirm it parses, then
        // write it into the input buffer with the codec.
        let parsed: ExtMessage = serde_json::from_value(request_json.clone())?;
        assert!(matches!(parsed, ExtMessage::RequestPlan { .. }));
        let mut input = Vec::new();
        codec::write_message(&mut input, &request_json)?;

        let mut reader = Cursor::new(input);
        let mut output: Vec<u8> = Vec::new();
        serve(&core, &mut reader, &mut output).await?;

        // The output stream is: a `hello` frame, then a `decoyPlan` frame.
        let mut out = Cursor::new(output);
        let hello: serde_json::Value = match codec::read_message(&mut out)? {
            Some(v) => v,
            None => anyhow::bail!("expected a hello frame"),
        };
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["schemaVersion"], PROTOCOL_VERSION);

        let plan: DecoyPlan = match codec::read_message(&mut out)? {
            Some(v) => v,
            None => anyhow::bail!("expected a decoyPlan frame"),
        };
        assert_eq!(plan.intent, REQUIRED_INTENT);
        assert_eq!(plan.persona_id, persona.id);
        assert_eq!(plan.seed, 1);
        assert!(plan.targets.len() <= 3);

        // No more frames: a clean EOF at the boundary.
        let trailing: Option<serde_json::Value> = codec::read_message(&mut out)?;
        assert!(trailing.is_none(), "no frames after the plan");
        Ok(())
    }
}
