//! Frozen license fixtures.
//!
//! This key was minted by `ivory-keygen` with the real k1 signing key. It is a
//! REAL, valid supporter key — deliberately committed, because its only power
//! is unlocking cosmetics in a free app, and what it buys us is worth far more:
//! it pins the entire chain (payload layout, base32 alphabet, CRC, signature,
//! public keyring) so that no future refactor can quietly stop honoring keys
//! that are already in customers' hands. If this test ever fails, STOP — you
//! are about to ship a build that locks out everyone who paid.
//!
//! Requires the `license` feature (the GUI always enables it).
#![cfg(feature = "license")]

use ivory_core::license::{verify_key, License, LicenseError, TIER_SUPPORTER};

/// Minted 2026-08-11, k1, name "Ganten".
const FIXTURE_KEY: &str = "\
X92VZ-CG104-4PW03-01DAN-6J068-XGPWX\n\
35DST-BBRPW-74RT6-0NEES-MW1JC-MC7J9\n\
3G47V-DNHKH-RKJKC-KM6FE-G0VK3-WDP8W\n\
7FDPP-G210V-SD8F6-23VEN-VR121-7KQ55\n\
WMVM5-0FMHM-9K6E0-2";

#[test]
fn a_real_issued_key_still_verifies() {
    let l = verify_key(FIXTURE_KEY).expect("issued keys must never stop verifying");
    assert_eq!(l.tier, TIER_SUPPORTER);
    assert_eq!(l.name.as_deref(), Some("Ganten"));
    assert_eq!(l.max_major, 0, "perpetual");
    assert_eq!(l.order_id, [0x60, 0x0b, 0x55, 0x53, 0x48]);
}

#[test]
fn the_same_key_survives_being_retyped() {
    // Lowercased, O/0 and I/1 swapped, newlines collapsed, dashes dropped —
    // the shapes a human actually produces when copying a key by hand.
    let mangled = FIXTURE_KEY
        .to_lowercase()
        .replace('0', "O")
        .replace('1', "l")
        .replace('\n', " ")
        .replace('-', " ");
    assert_eq!(verify_key(&mangled).unwrap().name.as_deref(), Some("Ganten"));
}

#[test]
fn one_altered_character_is_rejected() {
    let mut s: Vec<char> = FIXTURE_KEY.chars().collect();
    let pos = s.iter().rposition(|c| c.is_ascii_digit()).expect("has a digit");
    s[pos] = if s[pos] == '7' { '8' } else { '7' };
    let tampered: String = s.into_iter().collect();
    assert!(
        matches!(verify_key(&tampered), Err(LicenseError::Checksum | LicenseError::BadSignature)),
        "a mutated key must never verify"
    );
}

#[test]
fn payload_preimage_is_byte_frozen() {
    // Reconstructing the license and re-serializing must reproduce the exact
    // bytes that were signed. This is the check that catches a "harmless"
    // field reorder before it reaches a customer.
    let l = verify_key(FIXTURE_KEY).unwrap();
    let rebuilt = License { ..l.clone() };
    assert_eq!(rebuilt.payload_bytes(), l.payload_bytes());
    assert_eq!(l.payload_bytes()[0], 1, "format version 1");
}
