//! Guards the one manifest line that can silently disable supporter licenses.
//!
//! `ivory-core`'s `license` feature is non-default. If `ivory/Cargo.toml` ever
//! stops requesting it, verification compiles to a stub that returns false and
//! the shipped app rejects every real key — with the whole ivory-core test
//! suite still green, because that suite enables the feature itself. This test
//! runs in the GUI crate, so it sees exactly what ships.
use ivory_core::license::verify_key;

#[test]
fn the_shipped_binary_can_actually_verify_licenses() {
    // The committed fixture key (see ivory-core/tests/license_fixtures.rs).
    // With the feature off this returns Err(BadSignature) and the test fails.
    let key = include_str!("../../tests/fixtures/supporter.key");
    let l = verify_key(key).expect(
        "the GUI crate must enable ivory-core/license — without it every \
         supporter key is rejected at runtime",
    );
    assert_eq!(l.name.as_deref(), Some("Ganten"));
}
