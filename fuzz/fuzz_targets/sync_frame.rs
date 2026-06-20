// Coverage-guided fuzzer for the sealed sync-frame parser
// (fauxx_core::sync::SealedFrame::from_bytes). Invariant: parsing ARBITRARY
// transport bytes must never panic and must fail closed (an Ok or a typed Err,
// never a crash, slice-out-of-bounds, or plaintext leak). This is the wire-facing
// attack surface, so it is fuzzed directly on raw bytes.
#![no_main]

use fauxx_core::sync::SealedFrame;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = SealedFrame::from_bytes(data);
});
