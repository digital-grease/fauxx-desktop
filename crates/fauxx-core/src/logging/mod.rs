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

//! Persisted debug logging and a scrubbed, shareable export (the bug-report
//! path).
//!
//! The binaries write a rotating, bounded debug log to a local directory (see
//! [`init`]); nothing ever leaves the machine on its own (no telemetry). When a
//! user hits a bug or crash they run an EXPORT, which reads those local logs,
//! REDACTS personal/secret-shaped content, prepends a diagnostics header, and
//! writes one shareable file to attach to a GitHub issue.
//!
//! Redaction is "aggressive" by owner decision: home/username paths, IP and MAC
//! addresses, emails, UUIDs, key fingerprints, long hex / base64 blobs, and the
//! caller-supplied literals (persona ids and names) are replaced with typed
//! placeholders before the export is written. The on-disk raw log keeps full
//! fidelity for the user's own debugging; only the EXPORTED copy is scrubbed.

use std::path::PathBuf;

use regex::Regex;

use crate::error::{CoreError, Result};

/// Application identity for the per-OS log directory (mirrors the store's
/// `ProjectDirs` triple so logs sit beside the data dir under one namespace).
const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "DigitalGrease";
const APP_NAME: &str = "fauxx";

/// The log file name prefix the rolling appender writes under (e.g.
/// `fauxx.log.2026-06-18`). The export reads every file beginning with this.
pub const LOG_FILE_PREFIX: &str = "fauxx";
/// The log file name suffix.
pub const LOG_FILE_SUFFIX: &str = "log";
/// How many rotated daily log files to retain. Bounds the on-disk footprint.
pub const MAX_LOG_FILES: usize = 7;

/// Literals shorter than this are NOT redacted. With word-boundary matching a
/// 2-char name (`Jo`) is safe (it cannot shred `Joan`), but a 1-char literal is
/// dropped to avoid pathological redaction.
const MIN_LITERAL_LEN: usize = 2;

/// The per-OS directory the debug log is written to: `<data dir>/logs`. Created
/// if missing. Errors if no data directory can be resolved.
pub fn log_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .ok_or_else(|| CoreError::Logging("no OS data directory for the log dir".to_string()))?;
    let dir = dirs.data_dir().join("logs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoreError::Logging(format!("creating log dir {}: {e}", dir.display())))?;
    Ok(dir)
}

/// Initialize logging for a binary (the CLI or the GUI call this once at
/// startup). Installs:
///
/// - a human-readable layer to STDERR (filtered by `RUST_LOG`, default `info`),
///   matching the prior behavior, and
/// - a rotating, bounded DEBUG LOG FILE in [`log_dir`] (daily rotation, the last
///   [`MAX_LOG_FILES`] kept), written SYNCHRONOUSLY so a panic line reaches disk
///   before `panic = "abort"` tears the process down, and
/// - a PANIC HOOK that records the panic (message + location) to the log so a
///   crash is captured for the bug-report export.
///
/// Best-effort: if the log directory or file cannot be opened, it falls back to
/// stderr-only logging (logging never aborts the program). Safe to call once;
/// calling it a second time will panic like any double subscriber install.
pub fn init() {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = fmt::layer().with_writer(std::io::stderr);

    // The file layer is optional: a host with no writable data dir still logs to
    // stderr. `Option<Layer>` is itself a `Layer` (a no-op when `None`).
    let file_layer = match file_appender() {
        Ok(appender) => Some(fmt::layer().with_ansi(false).with_writer(appender)),
        Err(e) => {
            eprintln!("fauxx: debug log file unavailable ({e}); logging to stderr only");
            None
        }
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    install_panic_hook();
}

/// Build the synchronous rolling file appender: daily rotation under [`log_dir`],
/// keeping the last [`MAX_LOG_FILES`] files.
fn file_appender() -> Result<tracing_appender::rolling::RollingFileAppender> {
    let dir = log_dir()?;
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(&dir)
        .map_err(|e| CoreError::Logging(format!("building rolling log appender: {e}")))
}

/// Install a panic hook that records the panic to the log (so a crash shows up in
/// the bug-report export) and then chains to the previous hook (which prints the
/// panic / aborts as configured).
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        tracing::error!(
            target: "fauxx::panic",
            location = %location,
            "panic: {}",
            panic_message(info)
        );
        previous(info);
    }));
}

/// Extract a human-readable message from a panic payload.
fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// A diagnostics header for the export: the build version, OS/arch, capture time,
/// and the source log dir. Scrubbed along with the body (the dir path's home
/// segment is redacted), so it never leaks the username.
pub fn diagnostics_header() -> String {
    let when = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = log_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    format!(
        "# fauxx debug log export\n\
         # version: {}\n\
         # os: {} arch: {}\n\
         # captured_at_epoch_secs: {when}\n\
         # source: {dir}\n\
         # NOTE: scrubbed for public sharing (user paths, IPv4/IPv6, emails, keys, secrets, ids, URLs, and persona/device/peer names redacted)",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// A compiled redaction policy: caller-supplied literals (persona ids/names,
/// device/peer names, proxy hosts, the home directory and account name) plus the
/// fixed pattern set. Cheap to reuse across many lines.
pub struct Redactions {
    /// One word-boundary-aware regex per literal. Boundary-anchored (not raw
    /// substring) so a persona named "plan" does not shred "planned"/"planner";
    /// a literal whose edge char is non-word (a path starting with `/`) drops the
    /// boundary on that edge so it still matches.
    literal_res: Vec<Regex>,
    /// `(pattern, replacement)` pairs applied after the literals. A replacement
    /// may reference a capture group via `${1}` (the regex crate expands it).
    patterns: Vec<(Regex, &'static str)>,
}

impl Redactions {
    /// Build the policy. `literals` are exact strings to redact (persona ids and
    /// names, device/peer/host names, proxy hosts, the home dir and account
    /// name); empties and 1-char ones are dropped so the scrub cannot mangle
    /// unrelated text. The pattern set is fixed.
    pub fn new(literals: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut literals: Vec<String> = literals
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| s.chars().count() >= MIN_LITERAL_LEN)
            .collect();
        // Longest first so "Round Trip Persona" is redacted before "Round".
        literals.sort_by_key(|s| std::cmp::Reverse(s.len()));
        literals.dedup();

        let mut literal_res = Vec::with_capacity(literals.len());
        for lit in &literals {
            // Anchor with `\b` only where the literal's edge is a word char, so
            // names match on word boundaries while paths/hosts (non-word edges)
            // still match.
            let starts_word = lit
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let ends_word = lit
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let prefix = if starts_word { r"\b" } else { "" };
            let suffix = if ends_word { r"\b" } else { "" };
            let re = Regex::new(&format!("{prefix}{}{suffix}", regex::escape(lit)))
                .map_err(|e| CoreError::Logging(format!("compiling literal redaction: {e}")))?;
            literal_res.push(re);
        }

        // The fixed, aggressive pattern set. Ordered so a more specific / longer
        // form wins before a generic one, and so structured addresses are not
        // split. Each pattern is shaped to avoid eating useful, non-sensitive
        // tokens (timestamps like `12:34:56`, versions like `1.96.0`, Rust module
        // paths like `fauxx_cli::native_host`).
        let specs: &[(&str, &'static str)] = &[
            // key=value / key: value secrets, regardless of value length, so a
            // short token (`token=sk_live_x`) is caught even below the b64 floor.
            (
                r"(?i)\b(password|passwd|secret|token|api[-_]?key|authorization|bearer|access[-_]?key|credential|private[-_]?key)\b\s*[:=]\s*[^\s]+",
                "${1}=<redacted>",
            ),
            // URL path/query/fragment (may carry PII): keep scheme+host, drop the
            // rest. A bare origin (no path) is left intact (not sensitive).
            (r"(?i)(https?://[^\s/?#]+)[/?#][^\s]*", "${1}/<redacted>"),
            // Emails.
            (
                r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
                "<email>",
            ),
            // UUID v4-shaped ids (persona ids, plan ids).
            (
                r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
                "<uuid>",
            ),
            // User-home paths for ANY user (the home-dir literal only covers the
            // current user; a paired peer's path carries a different username).
            (r"(?i)C:\\Users\\[^\\/\s]+", r"C:\Users\<user>"),
            (r"/home/[^/\s]+", "/home/<user>"),
            (r"/Users/[^/\s]+", "/Users/<user>"),
            // mDNS / Bonjour host labels (device names that embed a real name,
            // e.g. `Jessicas-iPhone.local`).
            (r"\b[A-Za-z0-9][A-Za-z0-9._'\-]*\.local\b", "<host>.local"),
            // IPv6, BEFORE mac/fingerprint/ipv4 so an address is matched whole and
            // not split or mislabeled. We deliberately never match a bare `::`
            // (that would shred Rust module paths / log targets). Forms covered:
            // full 8-group; `::`-compressed with a hex group on at least one side
            // (with an optional embedded-IPv4 tail and an optional `%zone` id);
            // and a trailing-`::` prefix form.
            (
                r"\b(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}(?:%[0-9A-Za-z._\-]+)?",
                "<ipv6>",
            ),
            (
                r"\b[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{1,4})*::[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{1,4})*(?:\.\d{1,3})*(?:%[0-9A-Za-z._\-]+)?",
                "<ipv6>",
            ),
            (
                r"\b[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{1,4})*::(?:%[0-9A-Za-z._\-]+)?",
                "<ipv6>",
            ),
            // MAC address (six 2-hex groups, colon or dash separated; a time is
            // only three groups).
            (r"\b(?:[0-9a-fA-F]{2}[:\-]){5}[0-9a-fA-F]{2}\b", "<mac>"),
            // Device key fingerprint (four 4-hex groups, e.g. 1a2b:3c4d:5e6f:7081).
            (r"\b(?:[0-9a-fA-F]{4}:){3}[0-9a-fA-F]{4}\b", "<fingerprint>"),
            // IPv4 with 0-255 octets (so a `999.x` or a `1.96.0` version is not
            // mistaken for one).
            (
                r"\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b",
                "<ipv4>",
            ),
            // Long contiguous hex (keys, ciphertext, digests): >= 32 hex chars
            // (>= 16 bytes). A UUID has dashes, so it is not a 32-run.
            (r"\b[0-9a-fA-F]{32,}\b", "<hex>"),
            // Long base64url blobs (>= 40 chars: a 32-byte key is ~43). The class
            // excludes `/` and `+` so a long filesystem path is NOT eaten.
            (r"[A-Za-z0-9_\-]{40,}={0,2}", "<b64>"),
        ];
        let mut patterns = Vec::with_capacity(specs.len());
        for (pat, repl) in specs {
            let re = Regex::new(pat)
                .map_err(|e| CoreError::Logging(format!("compiling redaction pattern: {e}")))?;
            patterns.push((re, *repl));
        }
        Ok(Self {
            literal_res,
            patterns,
        })
    }

    /// Redact one line: word-boundary literals first (longest-first), then the
    /// pattern set.
    pub fn scrub_line(&self, line: &str) -> String {
        let mut out = line.to_string();
        for re in &self.literal_res {
            out = re.replace_all(&out, "<redacted>").into_owned();
        }
        for (re, repl) in &self.patterns {
            out = re.replace_all(&out, *repl).into_owned();
        }
        out
    }

    /// Scrub an entire multi-line blob.
    pub fn scrub_text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            out.push_str(&self.scrub_line(line));
            out.push('\n');
        }
        out
    }
}

/// Summary of an export: where it was written and how much it covered.
#[derive(Debug, Clone)]
pub struct ExportSummary {
    /// The shareable file written.
    pub out_path: PathBuf,
    /// Number of source log files read.
    pub files: usize,
    /// Number of log lines written (after scrubbing).
    pub lines: usize,
}

/// Export the persisted debug logs to `out_path` as one scrubbed, shareable
/// file: a diagnostics `header` (caller supplies version/OS/etc.), then every
/// retained log file's contents in chronological order, each line passed through
/// `redactions`. Returns a summary. Reads from [`log_dir`].
pub fn export(
    redactions: &Redactions,
    header: &str,
    out_path: &std::path::Path,
) -> Result<ExportSummary> {
    let dir = log_dir()?;
    // Collect the rolling files (prefix-matched) and read them oldest-first. The
    // daily appender names them `fauxx.log.YYYY-MM-DD`, so a lexical sort is
    // chronological.
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| CoreError::Logging(format!("reading log dir {}: {e}", dir.display())))?
    {
        let entry =
            entry.map_err(|e| CoreError::Logging(format!("reading a log dir entry: {e}")))?;
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with(LOG_FILE_PREFIX) && name.contains(LOG_FILE_SUFFIX) {
                files.push(path);
            }
        }
    }
    files.sort();

    let mut body = String::new();
    let mut lines = 0usize;
    for file in &files {
        let raw = std::fs::read_to_string(file)
            .map_err(|e| CoreError::Logging(format!("reading log file {}: {e}", file.display())))?;
        for line in raw.lines() {
            body.push_str(&redactions.scrub_line(line));
            body.push('\n');
            lines += 1;
        }
    }

    // The header is also scrubbed (it may carry a path), so nothing in the
    // exported file bypasses redaction.
    let scrubbed_header = redactions.scrub_text(header);
    let contents = format!("{scrubbed_header}\n{body}");
    std::fs::write(out_path, contents)
        .map_err(|e| CoreError::Logging(format!("writing export {}: {e}", out_path.display())))?;

    Ok(ExportSummary {
        out_path: out_path.to_path_buf(),
        files: files.len(),
        lines,
    })
}

/// The current user's home directory, if known, as a redaction literal. The
/// caller folds this into [`Redactions::new`] so absolute paths under it are
/// scrubbed even when the username is not otherwise guessable.
pub fn home_dir_literal() -> Option<String> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_string_lossy().into_owned())
}

/// Redaction literals identifying the local account: the home directory path,
/// its last component (the bare username), and `$USER` / `$USERNAME` / `$LOGNAME`.
/// Folded into the export literal set so the operator's account name is redacted
/// even when it appears outside a `/home/<user>` path.
pub fn account_literals() -> Vec<String> {
    let mut out = Vec::new();
    if let Some(dirs) = directories::BaseDirs::new() {
        let home = dirs.home_dir();
        out.push(home.to_string_lossy().into_owned());
        if let Some(base) = home.file_name().and_then(|n| n.to_str()) {
            out.push(base.to_string());
        }
    }
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                out.push(v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(literals: &[&str]) -> Redactions {
        match Redactions::new(literals.iter().map(|s| s.to_string())) {
            Ok(r) => r,
            Err(_) => Redactions {
                literal_res: Vec::new(),
                patterns: Vec::new(),
            },
        }
    }

    #[test]
    fn redacts_emails_uuids_and_addresses() {
        let r = red(&[]);
        let line = r.scrub_line(
            "user me@example.com persona aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa from 192.168.1.7",
        );
        assert!(line.contains("<email>"), "{line}");
        assert!(line.contains("<uuid>"), "{line}");
        assert!(line.contains("<ipv4>"), "{line}");
        assert!(!line.contains("me@example.com"));
        assert!(!line.contains("192.168.1.7"));
    }

    #[test]
    fn redacts_mac_fingerprint_and_long_secrets() {
        let r = red(&[]);
        let mac = r.scrub_line("nic 1a:2b:3c:4d:5e:6f up");
        assert!(mac.contains("<mac>"), "{mac}");
        let fp = r.scrub_line("peer fingerprint 1a2b:3c4d:5e6f:7081 paired");
        assert!(fp.contains("<fingerprint>"), "{fp}");
        let hex = r.scrub_line(&format!("ciphertext {} done", "4c4dd0992df13b4f".repeat(4)));
        assert!(hex.contains("<hex>"), "{hex}");
        let b64 = r.scrub_line("pk c7LYt2qptTZgAyvI9di-46OuTjs6f9Sa3oH3NHo0qmgABCDEFGH");
        assert!(b64.contains("<b64>"), "{b64}");
    }

    #[test]
    fn keeps_timestamps_and_versions_intact() {
        // The whole point of a debug log: do NOT eat the useful, non-sensitive
        // tokens. A wall-clock time and a semver must survive.
        let r = red(&[]);
        let line = r.scrub_line("2026-06-18T12:34:56Z fauxx 1.96.0 serve: tick 7");
        assert!(line.contains("12:34:56"), "time must survive: {line}");
        assert!(line.contains("1.96.0"), "version must survive: {line}");
        assert!(line.contains("tick 7"), "{line}");
    }

    #[test]
    fn redacts_caller_literals_on_word_boundaries_only() {
        let r = red(&["Round Trip Persona", "Jo"]);
        // A full multi-word literal is redacted...
        let line = r.scrub_line("loaded Round Trip Persona ok");
        assert!(line.contains("<redacted>"), "{line}");
        assert!(!line.contains("Round Trip Persona"));
        // ...but a literal must match on word boundaries: a standalone "Jo" is
        // redacted, while "Joan" (and unrelated words) are left intact.
        let mixed = r.scrub_line("Joan met Jo today");
        assert!(mixed.contains("Joan"), "no over-redaction of Joan: {mixed}");
        assert!(
            mixed.contains("<redacted>"),
            "standalone Jo redacted: {mixed}"
        );
        assert!(!mixed.contains(" Jo "), "{mixed}");
    }

    #[test]
    fn literal_does_not_shred_substrings() {
        // The over-redaction regression: a persona named "plan" must not eat
        // "planner"/"planned".
        let r = red(&["plan"]);
        let out = r.scrub_line("planner planned a plan now");
        assert!(out.contains("planner"), "{out}");
        assert!(out.contains("planned"), "{out}");
        assert!(out.contains("<redacted>"), "{out}");
        assert!(!out.contains(" plan "), "standalone plan redacted: {out}");
    }

    #[test]
    fn redacts_ipv6_zone_trailing_and_embedded_v4() {
        let r = red(&[]);
        // Zone id (link-local peer_addr): the %zone must not survive.
        let zoned = r.scrub_line("peer_addr=[fe80::1ff:fe23:4567:890a%eth0]:51820 up");
        assert!(zoned.contains("<ipv6>"), "{zoned}");
        assert!(!zoned.contains("eth0"), "zone id leaked: {zoned}");
        assert!(!zoned.contains("890a"), "{zoned}");
        // Trailing `::` prefix form (public-IP report path).
        let prefix = r.scrub_line("public_ip=2606:4700:: observed");
        assert!(prefix.contains("<ipv6>"), "{prefix}");
        assert!(!prefix.contains("2606"), "{prefix}");
        // Embedded IPv4 tail.
        let embedded = r.scrub_line("from 2001:db8::192.168.0.1 ok");
        assert!(!embedded.contains("168"), "embedded v4 leaked: {embedded}");
    }

    #[test]
    fn redacts_generic_user_paths_and_local_hosts() {
        let r = red(&[]);
        let win = r.scrub_line(r"failed to open C:\Users\jdoe\AppData\fauxx.db");
        assert!(!win.contains("jdoe"), "{win}");
        let nix = r.scrub_line("config /home/otheruser/.config/fauxx loaded");
        assert!(!nix.contains("otheruser"), "{nix}");
        let host = r.scrub_line("paired peer=jdoe-MacBook-Pro.local fingerprint ok");
        assert!(!host.contains("jdoe-MacBook-Pro"), "{host}");
        assert!(host.contains("<host>.local"), "{host}");
    }

    #[test]
    fn redacts_kv_secrets_and_url_paths() {
        let r = red(&[]);
        let secret = r.scrub_line("auth password=hunter2 token=sk_live_abc123 done");
        assert!(!secret.contains("hunter2"), "{secret}");
        assert!(!secret.contains("sk_live_abc123"), "{secret}");
        let url = r.scrub_line("GET https://broker.example.com/optout?email=me@x.io done");
        assert!(
            url.contains("https://broker.example.com"),
            "origin kept: {url}"
        );
        assert!(!url.contains("optout"), "{url}");
        assert!(!url.contains("me@x.io"), "{url}");
    }

    #[test]
    fn b64_pattern_does_not_eat_filesystem_paths() {
        let r = red(&[]);
        let line = r.scrub_line(
            "opened /var/lib/fauxx/some/long/nested/path/segments/here/store.sqlite ok",
        );
        assert!(!line.contains("<b64>"), "path eaten as b64: {line}");
        assert!(line.contains("store.sqlite"), "{line}");
    }

    #[test]
    fn redacts_home_paths_via_literal() {
        let r = red(&["/home/someuser"]);
        let line = r.scrub_line("opened /home/someuser/.local/share/fauxx/fauxx.db");
        assert!(!line.contains("someuser"), "{line}");
        assert!(line.contains("<redacted>"), "{line}");
    }

    #[test]
    fn redacts_ipv6_but_not_rust_paths_or_clocks() {
        let r = red(&[]);
        let full = r.scrub_line("from 2001:0db8:85a3:0000:0000:8a2e:0370:7334 ok");
        assert!(full.contains("<ipv6>"), "{full}");
        assert!(!full.contains("8a2e"), "full v6 fully redacted: {full}");
        let compressed = r.scrub_line("peer fe80::1ff:fe23:4567 seen");
        assert!(compressed.contains("<ipv6>"), "{compressed}");
        // CRITICAL regression: do NOT shred Rust module paths / log targets,
        // which are full of `::` and appear on nearly every log line.
        let path = r.scrub_line("target fauxx_cli::native_host dispatch ok");
        assert_eq!(path, "target fauxx_cli::native_host dispatch ok");
        // A wall-clock time is not an address.
        let clock = r.scrub_line("at 09:30:05 fired");
        assert!(clock.contains("09:30:05"), "a clock is not ipv6: {clock}");
    }

    #[test]
    fn scrub_text_handles_multiple_lines() {
        let r = red(&[]);
        let out = r.scrub_text("a me@x.io\nb 10.0.0.1\nc plain");
        assert!(out.contains("<email>"));
        assert!(out.contains("<ipv4>"));
        assert!(out.contains("c plain"));
        assert_eq!(out.lines().count(), 3);
    }
}
