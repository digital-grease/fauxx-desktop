// Coverage-guided fuzzer for the harmful-query blocklist
// (fauxx_core::querybank::QueryBlocklist::is_blocked). Invariant: matching
// ARBITRARY (incl. adversarial Unicode) input must never panic. NFKC folding plus
// regex matching is the surface; the property suite asserts the generated-query
// safety, while this target hunts for panics/timeouts on hostile bytes.
#![no_main]

use std::sync::OnceLock;

use fauxx_core::querybank::QueryBlocklist;
use libfuzzer_sys::fuzz_target;

// The bundled corpus compiles regexes; build it once.
static BLOCKLIST: OnceLock<QueryBlocklist> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let blocklist = BLOCKLIST.get_or_init(QueryBlocklist::bundled);
    if let Ok(query) = std::str::from_utf8(data) {
        let _ = blocklist.is_blocked(query);
    }
});
