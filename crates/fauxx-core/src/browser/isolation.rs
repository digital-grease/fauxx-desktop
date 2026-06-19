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

//! Strict separation guardrail (C2 #13, R3) for the decoy-profile launcher.
//!
//! This is a hard guardrail, enforced at profile launch and *fail closed*: a
//! decoy Chromium profile may run ONLY from a dedicated user-data directory
//! that is verifiably DISTINCT from any real browser profile on the machine,
//! and decoy automation may NEVER navigate to an authenticated-account sign-in
//! endpoint. If either invariant cannot be proven safe, the operation is
//! refused with a typed [`CoreError`] rather than allowed to proceed.
//!
//! The two checks here are pure and have no I/O beyond filesystem
//! canonicalization, so they are exercised by hermetic unit tests that inject
//! fake real-profile paths and decoy dirs (no browser launch required).
//!
//! Why a guardrail and not just a convention: the entire premise of the decoy
//! profile is that it is a throwaway identity. If it ever shared, imported, or
//! nested inside a logged-in profile, the synthetic Topics signal would
//! contaminate (or be attributed to) the real user, defeating the purpose and
//! leaking the real identity. So the launcher only ever creates and uses its
//! OWN directory; it has no code path that reads, copies, or imports cookies,
//! tokens, saved logins, or cache from a real profile.

use std::path::{Component, Path, PathBuf};

use crate::error::{CoreError, Result};

/// Known real-browser profile roots to keep the decoy strictly separate from,
/// per OS. The decoy user-data dir must not be, equal, or nest inside any of
/// these (in either direction), so the synthetic profile can never share state
/// with, or be mistaken for, a logged-in browser.
///
/// Paths are resolved against the user's home directory (Linux/macOS) or the
/// `LOCALAPPDATA` / `APPDATA` environment (Windows). Entries that cannot be
/// resolved on the current host are simply skipped.
pub fn known_real_profile_roots() -> Vec<PathBuf> {
    real_profile_roots_from(
        directories::BaseDirs::new().as_ref().map(|d| d.home_dir()),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
    )
}

/// The OS-keyed real-profile root list, parameterized over the host directories
/// so tests can drive it with synthetic homes without touching the real machine.
///
/// `home` is the user's home dir (Linux/macOS); `local_app_data` / `app_data`
/// are the Windows `%LOCALAPPDATA%` / `%APPDATA%` roots. Only the entries
/// relevant to the current target OS are produced.
fn real_profile_roots_from(
    home: Option<&Path>,
    local_app_data: Option<PathBuf>,
    app_data: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "linux")]
    if let Some(home) = home {
        // Chrome, Chromium, and Firefox on Linux.
        roots.push(home.join(".config/google-chrome"));
        roots.push(home.join(".config/chromium"));
        roots.push(home.join(".mozilla/firefox"));
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = home {
        roots.push(home.join("Library/Application Support/Google/Chrome"));
        roots.push(home.join("Library/Application Support/Firefox"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local) = &local_app_data {
            roots.push(local.join("Google").join("Chrome").join("User Data"));
        }
        if let Some(roaming) = &app_data {
            roots.push(roaming.join("Mozilla").join("Firefox"));
        }
    }

    // On the build's non-target OSes these inputs are unused; bind them so the
    // signature stays uniform and `-D warnings` stays quiet across platforms.
    let _ = (home, local_app_data, app_data);
    roots
}

/// Verify that `decoy_dir` is strictly separate from every path in
/// `real_roots`, returning the canonicalized decoy dir on success.
///
/// Fails closed with [`CoreError::Browser`] if the decoy dir equals, contains,
/// or is contained by any real profile root. The check is run in BOTH
/// directions (real-inside-decoy and decoy-inside-real) so neither nesting
/// arrangement can slip through.
///
/// Canonicalization resolves symlinks and `..` so a path cannot dodge the
/// prefix check by aliasing. Because a brand-new decoy dir may not exist yet,
/// the deepest existing ancestor is canonicalized and the not-yet-created tail
/// re-appended; the same is done for each real root (which likewise may be
/// absent on this host). This means the guard holds even before the directory
/// is created on disk.
pub fn ensure_isolated_from_real_profiles(
    decoy_dir: &Path,
    real_roots: &[PathBuf],
) -> Result<PathBuf> {
    // Fail closed on a non-absolute decoy dir. A relative path cannot be
    // meaningfully compared against the absolute real-profile roots (lexical
    // normalization keeps it relative), so the overlap check could pass a path
    // that in fact resolves inside a real profile once joined to the cwd.
    if !decoy_dir.is_absolute() {
        return Err(CoreError::Browser(format!(
            "refusing to launch: decoy profile dir {} must be an absolute path",
            decoy_dir.display()
        )));
    }

    let decoy = canonicalize_lexically(decoy_dir);

    for root in real_roots {
        let root = canonicalize_lexically(root);
        if paths_overlap(&decoy, &root) {
            return Err(CoreError::Browser(format!(
                "refusing to launch: decoy profile dir {} overlaps real browser profile {} \
                 (a decoy must never share, nest in, or contain a real profile)",
                decoy.display(),
                root.display()
            )));
        }
    }

    Ok(decoy)
}

/// Whether two canonical absolute paths overlap: equal, or one is a prefix
/// (ancestor) of the other. Component-wise so `/a/bc` is NOT treated as nested
/// under `/a/b` (a plain string prefix check would get that wrong).
fn paths_overlap(a: &Path, b: &Path) -> bool {
    is_prefix_of(a, b) || is_prefix_of(b, a)
}

/// Whether `ancestor` is `descendant` or one of its parents, compared
/// component-by-component (so partial final-segment matches do not count).
fn is_prefix_of(ancestor: &Path, descendant: &Path) -> bool {
    let mut a = ancestor.components();
    let mut d = descendant.components();
    loop {
        match (a.next(), d.next()) {
            // Ran out of ancestor components with all matched so far: ancestor
            // is the descendant itself or one of its parents.
            (None, _) => return true,
            (Some(ac), Some(dc)) if ac == dc => continue,
            _ => return false,
        }
    }
}

/// Canonicalize as much of `path` as exists on disk, re-appending any
/// not-yet-created tail. Falls back to lexical normalization when nothing on
/// the path exists yet (e.g. a fresh decoy dir, or a real root absent on this
/// host), so the overlap check is meaningful before the dir is created.
fn canonicalize_lexically(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }

    // Walk up to the deepest existing ancestor, canonicalize it, then re-append
    // the missing tail. This resolves symlinks/.. on the real part of the path
    // while still yielding an absolute, normalized path for the rest.
    let mut existing = path;
    let mut tail: Vec<Component> = Vec::new();
    loop {
        if existing.exists() {
            if let Ok(canon) = existing.canonicalize() {
                let mut out = canon;
                for comp in tail.iter().rev() {
                    out.push(comp.as_os_str());
                }
                return out;
            }
        }
        match existing.parent() {
            Some(parent) if parent != existing => {
                if let Some(name) = existing.file_name() {
                    tail.push(Component::Normal(name));
                }
                existing = parent;
            }
            _ => break,
        }
    }

    normalize_lexically(path)
}

/// Pure lexical normalization (no filesystem access): collapse `.` and resolve
/// `..` against earlier components. Used as the last-resort fallback when no
/// part of the path exists yet.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Authenticated-account sign-in endpoints decoy automation must NEVER drive.
///
/// Two things are out of scope for a *decoy* profile: it has no real account to
/// sign into, and driving a real sign-in flow would risk attaching the synthetic
/// activity to a genuine identity. The launcher refuses navigation to any of
/// these hosts/paths (matched on the URL's host and, where listed, its path
/// prefix) and locally logs the blocked attempt via `tracing`. Nothing about a
/// blocked attempt leaves the machine; the log is local only (no telemetry).
///
/// Each entry is `(host_suffix, optional path-prefix)`. The host match is a
/// suffix match on a dot boundary so `accounts.google.com` also covers
/// `accounts.google.com.` and subdomains, but not look-alikes like
/// `notaccounts.google.com.evil.test`.
const AUTH_FLOW_BLOCKLIST: &[(&str, Option<&str>)] = &[
    // Google account sign-in (the canonical sign-in hosts; this list is scoped
    // to authenticated sign-in surfaces, not every Google/Meta property).
    ("accounts.google.com", None),
    ("accounts.youtube.com", None),
    // Microsoft / Live account sign-in.
    ("login.live.com", None),
    ("login.microsoft.com", None),
    ("login.microsoftonline.com", None),
    // Meta / Facebook login (bare host and the explicit login paths).
    ("facebook.com", Some("/login")),
    ("www.facebook.com", Some("/login")),
    ("facebook.com", Some("/login.php")),
    ("www.facebook.com", Some("/login.php")),
    // Apple ID sign-in.
    ("appleid.apple.com", None),
];

/// Whether `url` is an authenticated-account endpoint the decoy must not drive.
///
/// Parses the URL's authority and path with a minimal, dependency-free parser
/// (we only need scheme/host/path, and adding a URL crate is unwarranted) and
/// matches against [`AUTH_FLOW_BLOCKLIST`]. A URL we cannot parse a host from
/// (e.g. `about:blank`, `data:`) is NOT on the blocklist and is allowed; those
/// are exactly the safe local URLs the live test falls back to.
pub fn is_blocked_auth_flow(url: &str) -> bool {
    let Some((host, path)) = host_and_path(url) else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    // Match the path case-insensitively too, so `/LOGIN` is blocked the same as
    // `/login` (the blocklist prefixes are lowercase).
    let path = path.to_ascii_lowercase();

    AUTH_FLOW_BLOCKLIST
        .iter()
        .any(|(blocked_host, blocked_path)| {
            let host_matches = host == *blocked_host || host.ends_with(&format!(".{blocked_host}"));
            if !host_matches {
                return false;
            }
            match blocked_path {
                None => true,
                Some(prefix) => path.starts_with(prefix),
            }
        })
}

/// Guard a navigation target: returns `Ok` if it is allowed, or a typed
/// [`CoreError::Browser`] (after a local `tracing` warning) if it is non-loopback
/// plaintext HTTP or a blocked authenticated-account endpoint. The warning is
/// local only.
///
/// Plaintext `http://` to a PUBLIC host is refused: a decoy visit over cleartext
/// is observable and injectable on the wire, which defeats the point. Plaintext
/// HTTP to a LOOPBACK host (`127.0.0.0/8`, `localhost`, `::1`) is allowed: it
/// never reaches the wire, so the observe/inject rationale does not apply, and a
/// local server (a test fixture, a local proxy front) legitimately uses it.
/// `https://` and the schemeless local fallbacks the live test uses
/// (`about:blank`, `data:`) are allowed; the latter carry no host, so the
/// auth-flow blocklist ignores them.
pub fn ensure_navigation_allowed(url: &str) -> Result<()> {
    if url.starts_with("http://") && !is_loopback_http(url) {
        tracing::warn!(
            target: "fauxx_core::browser::isolation",
            blocked_url = %url,
            "refused decoy navigation over plaintext HTTP (HTTPS-only)"
        );
        return Err(CoreError::Browser(format!(
            "refusing decoy navigation over plaintext HTTP: {url} \
             (decoy traffic must be HTTPS so it cannot be observed or injected)"
        )));
    }
    if is_blocked_auth_flow(url) {
        // Local-only log. No telemetry leaves the machine; this is a tracing
        // event the operator can see in their own logs.
        tracing::warn!(
            target: "fauxx_core::browser::isolation",
            blocked_url = %url,
            "refused decoy navigation to an authenticated-account endpoint"
        );
        return Err(CoreError::Browser(format!(
            "refusing decoy navigation to authenticated-account endpoint: {url} \
             (decoy automation must never drive real sign-in flows)"
        )));
    }
    Ok(())
}

/// Whether `url` is plaintext HTTP to a LOOPBACK host (so it never hits the wire
/// and the HTTPS-only rule does not apply): `localhost`, an IPv4 address in
/// `127.0.0.0/8`, or the IPv6 `::1` (with or without a port / userinfo). Parses
/// the authority directly so an IPv6 literal in brackets is handled (which
/// [`host_and_path`] does not), and uses [`std::net::IpAddr::is_loopback`] so a
/// mere `127`-prefixed DOMAIN (e.g. `127.example.com`) is NOT treated as loopback.
fn is_loopback_http(url: &str) -> bool {
    let Some(after_scheme) = url.strip_prefix("http://") else {
        return false;
    };
    let auth_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..auth_end];
    // Drop any userinfo ("user@").
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // Separate host from port, handling a bracketed `[IPv6]:port` literal.
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    let host = host.to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Split a URL into `(host, path)` without pulling in a URL crate. Returns
/// `None` for URLs with no authority (e.g. `about:`, `data:`, `file:`-less),
/// which the caller treats as "not a blocked auth host".
fn host_and_path(url: &str) -> Option<(String, String)> {
    // Strip the scheme up to "://"; schemes without an authority (about:, data:)
    // have no "//" and yield None.
    let after_scheme = url.split_once("://")?.1;
    // Authority ends at the first '/', '?' or '#'.
    let auth_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(auth_end);
    // Drop any userinfo ("user:pass@") and any port (":443").
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port.split(':').next().unwrap_or(host_port);
    if host.is_empty() {
        return None;
    }
    // The path is the leading part of `rest` up to a query/fragment.
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = if rest.is_empty() {
        "/".to_string()
    } else {
        rest[..path_end].to_string()
    };
    Some((host.to_string(), path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_paths_overlap_and_are_refused() {
        let real = PathBuf::from("/home/u/.config/google-chrome");
        let decoy = PathBuf::from("/home/u/.config/google-chrome");
        assert!(matches!(
            ensure_isolated_from_real_profiles(&decoy, &[real]),
            Err(CoreError::Browser(_))
        ));
    }

    #[test]
    fn decoy_nested_inside_real_is_refused() {
        // Decoy dir lives *inside* a real Chrome profile: the worst case.
        let real = PathBuf::from("/home/u/.config/google-chrome");
        let decoy = PathBuf::from("/home/u/.config/google-chrome/Default/decoy");
        assert!(matches!(
            ensure_isolated_from_real_profiles(&decoy, &[real]),
            Err(CoreError::Browser(_))
        ));
    }

    #[test]
    fn real_nested_inside_decoy_is_refused() {
        // The reverse nesting must also be refused (decoy is an ancestor of a
        // real profile root), so we never wrap a logged-in profile either.
        let real = PathBuf::from("/home/u/data/decoy-profiles/google-chrome");
        let decoy = PathBuf::from("/home/u/data/decoy-profiles");
        assert!(matches!(
            ensure_isolated_from_real_profiles(&decoy, &[real]),
            Err(CoreError::Browser(_))
        ));
    }

    #[test]
    fn sibling_with_shared_prefix_string_is_allowed() {
        // `.../google-chrome-decoy` must NOT be treated as nested under
        // `.../google-chrome` despite the shared string prefix.
        let real = PathBuf::from("/home/u/.config/google-chrome");
        let decoy = PathBuf::from("/home/u/.config/google-chrome-decoy");
        assert!(ensure_isolated_from_real_profiles(&decoy, &[real]).is_ok());
    }

    #[test]
    fn distinct_decoy_dir_is_allowed() {
        let real = PathBuf::from("/home/u/.config/chromium");
        let decoy = PathBuf::from("/home/u/.local/share/fauxx/decoy-profiles/abc");
        let ok = ensure_isolated_from_real_profiles(&decoy, &[real]);
        assert!(ok.is_ok(), "distinct dir should be allowed: {ok:?}");
    }

    #[test]
    fn relative_decoy_dir_is_refused() {
        // A relative path cannot be compared against absolute real roots, so it
        // must fail closed rather than slip through the overlap check.
        let real = PathBuf::from("/home/u/.config/google-chrome");
        let decoy = PathBuf::from("relative/decoy");
        assert!(matches!(
            ensure_isolated_from_real_profiles(&decoy, &[real]),
            Err(CoreError::Browser(_))
        ));
    }

    #[test]
    fn refused_against_the_full_known_real_root_set() {
        // Inject a synthetic Linux home and assert the real roots are derived
        // and a decoy placed inside one is refused.
        let home = Path::new("/home/synthetic");
        let roots = real_profile_roots_from(Some(home), None, None);
        #[cfg(target_os = "linux")]
        {
            assert!(roots.contains(&home.join(".config/google-chrome")));
            assert!(roots.contains(&home.join(".config/chromium")));
            assert!(roots.contains(&home.join(".mozilla/firefox")));
            let inside = home.join(".mozilla/firefox/abcd.default/decoy");
            assert!(matches!(
                ensure_isolated_from_real_profiles(&inside, &roots),
                Err(CoreError::Browser(_))
            ));
        }
        #[cfg(not(target_os = "linux"))]
        let _ = roots;
    }

    #[test]
    fn blocklist_rejects_known_auth_endpoints() {
        for url in [
            "https://accounts.google.com/signin/v2/identifier",
            "https://accounts.google.com",
            "https://login.live.com/",
            "https://login.microsoftonline.com/common/oauth2",
            "https://www.facebook.com/login.php?next=x",
            "https://facebook.com/login",
            "https://appleid.apple.com/auth/authorize",
            // Subdomain of a blocked host is also blocked.
            "https://mail.accounts.google.com/",
        ] {
            assert!(is_blocked_auth_flow(url), "should block: {url}");
            assert!(
                ensure_navigation_allowed(url).is_err(),
                "should refuse: {url}"
            );
        }
    }

    #[test]
    fn blocklist_allows_ordinary_decoy_urls() {
        for url in [
            "https://news.ycombinator.com/",
            "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            // Facebook NON-login paths are allowed (we only block sign-in).
            "https://www.facebook.com/some-public-page",
            // (the uppercase-path case is asserted blocked in a separate test)
            // A look-alike host that merely contains the blocked string is NOT
            // a subdomain of it, so it is allowed (must not over-block).
            "https://notaccounts.google.com.evil.test/",
            "https://accounts.google.com.evil.test/",
            // Local fallbacks the live test uses have no host and are allowed.
            "about:blank",
            "data:text/html,<h1>hi</h1>",
        ] {
            assert!(!is_blocked_auth_flow(url), "should allow: {url}");
            assert!(
                ensure_navigation_allowed(url).is_ok(),
                "should allow: {url}"
            );
        }
    }

    #[test]
    fn plaintext_http_navigation_is_refused() {
        // Cleartext HTTP is observable/injectable on the wire; the decoy is
        // HTTPS-only even for an otherwise-allowed (non-auth) host.
        for url in [
            "http://news.ycombinator.com/",
            "http://en.wikipedia.org/wiki/Rust",
        ] {
            assert!(
                ensure_navigation_allowed(url).is_err(),
                "plaintext HTTP must be refused: {url}"
            );
            // The HTTPS form of the same host stays allowed.
            let https = url.replacen("http://", "https://", 1);
            assert!(
                ensure_navigation_allowed(&https).is_ok(),
                "HTTPS form should be allowed: {https}"
            );
        }
    }

    #[test]
    fn loopback_plaintext_http_is_allowed() {
        // Loopback never hits the wire, so the HTTPS-only rationale does not
        // apply: a local test server / proxy front may be plain http. (The live
        // GPC test navigates a 127.0.0.1 loopback server to inspect the header.)
        for url in [
            "http://127.0.0.1:8080/",
            "http://127.0.0.1/.well-known/gpc.json",
            "http://localhost:3000/",
            "http://[::1]:9000/",
        ] {
            assert!(
                ensure_navigation_allowed(url).is_ok(),
                "loopback plaintext HTTP should be allowed: {url}"
            );
        }
        // A non-loopback host that merely starts with "127" in a label is NOT
        // loopback (e.g. a domain), so it stays refused.
        assert!(ensure_navigation_allowed("http://127.example.com/").is_err());
    }

    #[test]
    fn blocklist_path_match_is_case_insensitive() {
        // An uppercased login path must still be blocked.
        for url in [
            "https://www.facebook.com/LOGIN",
            "https://facebook.com/Login.php?next=x",
        ] {
            assert!(is_blocked_auth_flow(url), "should block: {url}");
        }
    }

    #[test]
    fn host_and_path_parses_authority_and_path() {
        assert_eq!(
            host_and_path("https://user:pw@www.facebook.com:443/login.php?x=1"),
            Some(("www.facebook.com".to_string(), "/login.php".to_string()))
        );
        assert_eq!(
            host_and_path("https://example.com"),
            Some(("example.com".to_string(), "/".to_string()))
        );
        assert_eq!(host_and_path("about:blank"), None);
        assert_eq!(host_and_path("data:text/plain,hi"), None);
    }
}
