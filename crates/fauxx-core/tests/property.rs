//! Property-based (proptest) tests for the security-critical parsers. These
//! assert the invariants a privacy tool must never violate, across thousands of
//! generated inputs with shrinking:
//!
//!   - the log scrubber NEVER leaks a seeded secret and NEVER panics,
//!   - the sealed sync channel round-trips, fails closed on a wrong/forged
//!     sender or a tampered ciphertext, and the frame parser never panics on
//!     arbitrary bytes (always Ok or a fail-closed Err),
//!   - the harmful-query blocklist never panics on adversarial Unicode.
//!
//! Unit tests already cover specific known cases; these widen the input space.

use fauxx_core::logging::Redactions;
use fauxx_core::querybank::QueryBlocklist;
use fauxx_core::sync::{DeviceIdentity, SealedFrame};
use proptest::prelude::*;

/// Map a fauxx_core error into a proptest failure. The workspace lints flag
/// `unwrap`/`expect`, so tests thread Results through `?` instead.
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(e.to_string())
}

// ---------------------------------------------------------------------------
// Log scrubber (fauxx_core::logging::Redactions)
// ---------------------------------------------------------------------------
proptest! {
    /// Arbitrary input never panics the scrubber.
    #[test]
    fn scrubber_never_panics(line in ".*") {
        let r = Redactions::new(Vec::<String>::new()).map_err(fail)?;
        let _ = r.scrub_line(&line);
    }

    /// A seeded secret literal, present as a standalone (word-bounded) token in a
    /// line, is never present verbatim in the scrubbed output. The literal is
    /// uppercase/digits/underscore (4+ chars): that keeps it clear of the
    /// numeric/IP redaction families AND guarantees it cannot appear as an
    /// incidental substring of the lowercase wrapper words or the "<redacted>"
    /// placeholder (which would be a false "leak", not a real one).
    #[test]
    fn scrubber_never_leaks_seeded_literal(secret in "[A-Z][A-Z0-9_]{3,30}") {
        let r = Redactions::new([secret.clone()]).map_err(fail)?;
        // Word-delimited on both sides (the scrubber redacts seeded literals at
        // token boundaries, by design, to avoid eating substrings of real words).
        let line = format!("audit token={secret} status=ok");
        let scrubbed = r.scrub_line(&line);
        prop_assert!(
            !scrubbed.contains(&secret),
            "scrubbed output leaked the seeded secret {secret:?}: {scrubbed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Sealed sync frame + crypto (fauxx_core::sync)
// ---------------------------------------------------------------------------
proptest! {
    /// seal -> frame to_bytes -> from_bytes -> open round-trips any plaintext.
    #[test]
    fn sync_frame_roundtrips(plaintext in prop::collection::vec(any::<u8>(), 0..2048)) {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let envelope = alice.seal(bob.public_key(), &plaintext).map_err(fail)?;
        let bytes = SealedFrame { envelope }.to_bytes();
        let parsed = SealedFrame::from_bytes(&bytes).map_err(fail)?;
        let opened = bob.open(alice.public_key(), &parsed.envelope).map_err(fail)?;
        prop_assert_eq!(opened, plaintext);
    }

    /// The frame parser never panics on arbitrary bytes: it returns Ok or a
    /// fail-closed Err, but never aborts.
    #[test]
    fn sync_frame_parser_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let _ = SealedFrame::from_bytes(&bytes);
    }

    /// Opening with the WRONG (unpaired) sender key fails: the MAC is checked
    /// against the claimed sender, so an unpaired peer cannot decrypt.
    #[test]
    fn sync_open_fails_for_wrong_sender(plaintext in prop::collection::vec(any::<u8>(), 1..512)) {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let eve = DeviceIdentity::generate();
        let envelope = alice.seal(bob.public_key(), &plaintext).map_err(fail)?;
        prop_assert!(
            bob.open(eve.public_key(), &envelope).is_err(),
            "open accepted a frame attributed to the wrong sender"
        );
    }

    /// A single flipped byte anywhere in the ciphertext fails authentication
    /// (never silently decrypts tampered data).
    #[test]
    fn sync_tampered_ciphertext_fails(
        plaintext in prop::collection::vec(any::<u8>(), 1..512),
        idx in any::<usize>(),
    ) {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let mut envelope = alice.seal(bob.public_key(), &plaintext).map_err(fail)?;
        let i = idx % envelope.ciphertext.len();
        envelope.ciphertext[i] ^= 0xff;
        prop_assert!(
            bob.open(alice.public_key(), &envelope).is_err(),
            "open accepted a tampered ciphertext"
        );
    }
}

// ---------------------------------------------------------------------------
// Harmful-query blocklist (fauxx_core::querybank::QueryBlocklist)
// ---------------------------------------------------------------------------
proptest! {
    /// The blocklist matcher never panics on arbitrary Unicode (NFKC folding +
    /// regex matching must be robust against adversarial input).
    #[test]
    fn blocklist_never_panics(query in ".*") {
        let bl = QueryBlocklist::bundled();
        let _ = bl.is_blocked(&query);
    }
}
