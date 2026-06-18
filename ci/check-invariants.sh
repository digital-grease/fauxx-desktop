#!/usr/bin/env bash
# Invariant grep-checks enforced in CI.
#
# Each check maps to a non-negotiable invariant from the project ethos:
# 100% local / no telemetry, no unsafe, rustls over OpenSSL, decoy-profile
# isolation, no credential automation against real accounts, and the docs
# style rule (no em-dashes). These are coarse at C0 and tighten as the
# browser (C2) and deterministic-channel (C3) milestones land.

set -euo pipefail

fail() {
    echo "[INVARIANT FAIL] $*" >&2
    exit 1
}

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

# -----------------------------------------------------------------------------
# 1. Zero-telemetry invariant.
#    No analytics / telemetry / crash-reporting SDKs anywhere in the tree. The
#    app is 100% local with no backend and no telemetry. (HTTP clients such as
#    reqwest are legitimately used for lawful broker opt-outs and DSAR, so this
#    targets the telemetry SDKs specifically rather than HTTP in general.)
# -----------------------------------------------------------------------------
echo "==> Check 1: no telemetry / analytics SDKs"
telemetry_hits=$(grep -RInE '^\s*(sentry|sentry-[a-z-]+|segment|analytics|posthog|mixpanel|amplitude|rudderanalytics|datadog|opentelemetry-otlp|google-analytics|ga4|heap|countly)\s*=' \
    --include='Cargo.toml' . 2>/dev/null || true)
if [ -n "$telemetry_hits" ]; then
    echo "$telemetry_hits"
    fail "telemetry / analytics dependency found; the app must remain 100% local with no telemetry."
fi

# -----------------------------------------------------------------------------
# 2. No unsafe code (workspace lints forbid it; this is a structural backstop
#    in case a crate ever forgets `[lints] workspace = true`).
# -----------------------------------------------------------------------------
echo "==> Check 2: no unsafe{} in first-party source"
unsafe_hits=$(grep -RnE '\bunsafe\s*\{' crates apps --include='*.rs' 2>/dev/null || true)
if [ -n "$unsafe_hits" ]; then
    echo "$unsafe_hits"
    fail "unsafe{} block in first-party source; unsafe_code is forbidden workspace-wide."
fi

# -----------------------------------------------------------------------------
# 3. rustls over OpenSSL / native-tls (also enforced by deny.toml bans; kept
#    here for a fast, dependency-resolution-free signal).
# -----------------------------------------------------------------------------
echo "==> Check 3: no openssl / native-tls direct dependency"
tls_hits=$(grep -RInE '^\s*(openssl|openssl-sys|native-tls)\s*=' \
    --include='Cargo.toml' . 2>/dev/null || true)
if [ -n "$tls_hits" ]; then
    echo "$tls_hits"
    fail "openssl / native-tls dependency found; prefer rustls / RustCrypto."
fi

# -----------------------------------------------------------------------------
# 4. Decoy-profile isolation + no credential automation against real accounts.
#    ADVISORY at C0: there is no browser-automation code yet. Tightened into a
#    real surface audit when C2 (#11 R1 / #13 R3) and C3 land: the decoy profile
#    must launch only from a dedicated user-data dir and must never import real
#    cookies/tokens/logins or drive authenticated account flows.
# -----------------------------------------------------------------------------
echo "==> Check 4: decoy-profile isolation / no credential automation (advisory until C2/C3)"

# -----------------------------------------------------------------------------
# 5. Docs style: no em-dashes in tracked Markdown. Use commas or restructure
#    instead.
# -----------------------------------------------------------------------------
echo "==> Check 5: no em-dashes in tracked Markdown"
if command -v git >/dev/null 2>&1 && git rev-parse --git-dir >/dev/null 2>&1; then
    md_files=$(git ls-files '*.md' 2>/dev/null || true)
else
    md_files=$(find . -name '*.md' -not -path './target/*' 2>/dev/null || true)
fi
if [ -n "$md_files" ]; then
    emdash_hits=$(printf '%s\n' "$md_files" | xargs grep -Hn '—' 2>/dev/null || true)
    if [ -n "$emdash_hits" ]; then
        echo "$emdash_hits"
        fail "em-dash found in tracked Markdown; use commas or restructure."
    fi
fi

echo "All invariant checks passed."
