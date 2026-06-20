// Coverage-guided fuzzer for the log scrubber (fauxx_core::logging::Redactions).
// Invariant: scrubbing arbitrary input must never panic. The seeded literals let
// libfuzzer explore inputs that brush against the redaction patterns; the
// property suite (tests/property.rs) asserts the never-leak guarantee, while this
// target hammers the regex engine for panics/timeouts on adversarial bytes.
#![no_main]

use std::sync::OnceLock;

use fauxx_core::logging::Redactions;
use libfuzzer_sys::fuzz_target;

// Building the per-literal regex set is expensive; do it once.
static REDACTIONS: OnceLock<Redactions> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let redactions = REDACTIONS.get_or_init(|| {
        Redactions::new([
            "SeededSecretToken".to_string(),
            "someuser".to_string(),
            "alice@example.com".to_string(),
            "192.168.1.50".to_string(),
        ])
        .expect("build redactions")
    });
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = redactions.scrub_line(text);
        let _ = redactions.scrub_text(text);
    }
});
