//! Automated Gumroad fulfilment: a sale arrives, a supporter key goes out.
//!
//! Gumroad Ping POSTs to `/hook/<SECRET>` on every sale. This service verifies
//! the sale is real, mints an Ivory key with the SAME encoder the app verifies
//! with, emails it to the buyer, and records it. No manual step, ever.
//!
//! Deliberately boring and defensive, because it runs unattended and handles
//! money-adjacent events:
//!   * **Idempotent.** A sale id already fulfilled re-sends the ORIGINAL key
//!     rather than minting a second one. Gumroad retries pings, and a buyer who
//!     gets two different keys will reasonably think one is broken.
//!   * **Verified.** The ping is checked against Gumroad's API before anything
//!     is minted; a bare POST to the URL is not enough. Anyone can guess a URL.
//!   * **Never loses a key.** The ledger is written BEFORE the email is
//!     attempted, so a mail outage can never lose a key that a buyer paid for —
//!     it can be re-sent from the ledger.
//!
//! Environment:
//!   IVORY_SIGNING_SEED   hex seed of the signing key (use k2; keep k1 offline)
//!   IVORY_HOOK_SECRET    random path segment, e.g. from `openssl rand -hex 16`
//!   GUMROAD_TOKEN        Gumroad access token, to verify sales
//!   GUMROAD_SELLER_ID    your seller id, cheap first-pass rejection
//!   RESEND_API_KEY       email provider key
//!   MAIL_FROM            e.g. "Ivory <keys@yourdomain>"
//!   LEDGER_PATH          default /data/ledger.jsonl (persist this volume!)
//!   PORT                 default 8080

use ed25519_compact::{KeyPair, Seed};
use ivory_core::license::{encode_key, verify_key, License, TIER_SUPPORTER};
use std::collections::HashMap;

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        eprintln!("missing required environment variable {key}");
        std::process::exit(2);
    })
}

fn env_opt(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Days since 2020-01-01 (display only; nothing expires).
fn issued_days_now() -> u16 {
    const EPOCH_2020: u64 = 1_577_836_800;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (secs.saturating_sub(EPOCH_2020) / 86_400).min(u16::MAX as u64) as u16
}

/// `application/x-www-form-urlencoded` -> map. Gumroad Ping posts this shape.
fn parse_form(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in body.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        out.insert(url_decode(k), url_decode(v));
    }
    out
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hi = (b[i + 1] as char).to_digit(16);
                let lo = (b[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A previously fulfilled sale, recovered from the ledger.
fn ledger_lookup(path: &str, sale_id: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v.get("sale_id").and_then(|s| s.as_str()) == Some(sale_id) {
            return v
                .get("key")
                .and_then(|s| s.as_str())
                .map(str::to_owned);
        }
    }
    None
}

fn ledger_append(path: &str, entry: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{entry}")
}

/// Ask Gumroad whether this sale is real. Without this, anyone who guesses the
/// hook URL mints themselves keys.
fn sale_is_genuine(token: &str, sale_id: &str) -> bool {
    let url = format!("https://api.gumroad.com/v2/sales/{sale_id}");
    match ureq::get(&url)
        .query("access_token", token)
        .timeout(std::time::Duration::from_secs(15))
        .call()
    {
        Ok(resp) => match resp.into_json::<serde_json::Value>() {
            Ok(v) => v.get("success").and_then(|s| s.as_bool()).unwrap_or(false),
            Err(e) => {
                eprintln!("gumroad verify: bad JSON: {e}");
                false
            }
        },
        Err(e) => {
            eprintln!("gumroad verify: request failed: {e}");
            false
        }
    }
}

// Permanent download links. GitHub resolves `releases/latest/download/<name>`
// by EXACT asset name against whatever release is currently latest, so these
// stay correct across every future release — but ONLY for as long as
// `scripts/publish-github.sh` keeps uploading the version-less alias assets.
// Never pin these to a version: buyers keep this email for years, and a link
// to 2.2.0 in 2029 is worse than no link at all.
const DOWNLOAD_MACOS: &str =
    "https://github.com/ganten1998/ivory/releases/latest/download/Ivory-macos-arm64.dmg";
const DOWNLOAD_WINDOWS: &str =
    "https://github.com/ganten1998/ivory/releases/latest/download/ivory-windows-x86_64.zip";
const DOWNLOAD_LINUX: &str =
    "https://github.com/ganten1998/ivory/releases/latest/download/ivory-linux-x86_64.tar.gz";

/// Split out from `send_email` purely so it can be rendered and asserted on in
/// a test. The one email a buyer ever receives is not a good place to discover
/// a formatting mistake.
fn email_body(key: &str) -> String {
    format!(
        "Thank you for supporting Ivory.\n\n\
         Ivory is free and stays free. This key is a thank-you, not an unlock.\n\n\
         Your supporter key:\n\n\
         {key}\n\n\
         To use it: open Ivory, right-click anywhere, choose \"Support Ivory...\",\n\
         paste the key and press Activate. Case, spaces, dashes and line breaks\n\
         do not matter.\n\n\
         Downloads. These links always give you the current version, so they are\n\
         worth keeping alongside the key:\n\n\
         \x20 macOS 11 or later (Apple Silicon)\n\
         \x20 {macos}\n\n\
         \x20 Windows 10 or later\n\
         \x20 {windows}\n\n\
         \x20 Linux x86_64\n\
         \x20 {linux}\n\n\
         Keep this email. The key has no expiry and works on every machine you own.\n\n\
         Thanks again,\n\
         Ivory\n",
        macos = DOWNLOAD_MACOS,
        windows = DOWNLOAD_WINDOWS,
        linux = DOWNLOAD_LINUX,
    )
}

fn send_email(api_key: &str, from: &str, to: &str, name: &str, key: &str) -> Result<(), String> {
    let body = email_body(key);
    let payload = serde_json::json!({
        "from": from,
        "to": [to],
        "subject": "Your Ivory supporter key",
        "text": body,
        "reply_to": from,
    });
    let _ = name; // reserved: personalised subject lines
    ureq::post("https://api.resend.com/emails")
        .set("Authorization", &format!("Bearer {api_key}"))
        .timeout(std::time::Duration::from_secs(20))
        .send_json(payload)
        .map(|_| ())
        .map_err(|e| format!("resend: {e}"))
}

fn mint(kp: &KeyPair, name: Option<&str>) -> Result<(String, [u8; 5]), String> {
    // 40 bits of order id from the OS RNG (via a throwaway keypair; this binary
    // already depends on `random`, so no extra dependency for it).
    let entropy = KeyPair::generate();
    let mut order_id = [0u8; 5];
    order_id.copy_from_slice(&entropy.pk.as_ref()[..5]);

    let license = License {
        tier: TIER_SUPPORTER,
        issued_days: issued_days_now(),
        max_major: 0, // perpetual
        order_id,
        name: name.map(str::to_owned),
    };
    let payload = license.payload_bytes();
    let sig = kp.sk.sign(&payload, None);
    let mut sig64 = [0u8; 64];
    sig64.copy_from_slice(sig.as_ref());
    let key = encode_key(&payload, &sig64);

    // Never send a key that does not verify. Cheap, and the one check that
    // guarantees a buyer is not emailed something broken.
    match verify_key(&key) {
        Ok(v) if v == license => Ok((key, order_id)),
        Ok(_) => Err("minted key round-tripped to a different licence".into()),
        Err(e) => Err(format!("minted key does not verify: {e:?}")),
    }
}

fn main() {
    let seed_hex = env("IVORY_SIGNING_SEED");
    let hook_secret = env("IVORY_HOOK_SECRET");
    let gumroad_token = env("GUMROAD_TOKEN");
    let seller_id = env_opt("GUMROAD_SELLER_ID", "");
    let resend_key = env("RESEND_API_KEY");
    let mail_from = env("MAIL_FROM");
    let ledger = env_opt("LEDGER_PATH", "/data/ledger.jsonl");
    let port = env_opt("PORT", "8080");

    let seed_bytes = hex_decode(&seed_hex).unwrap_or_else(|| {
        eprintln!("IVORY_SIGNING_SEED is not hex");
        std::process::exit(2);
    });
    let seed = Seed::from_slice(&seed_bytes).unwrap_or_else(|_| {
        eprintln!("IVORY_SIGNING_SEED is the wrong length");
        std::process::exit(2);
    });
    let keypair = KeyPair::from_seed(seed);

    // Fail fast at boot rather than on the first real sale.
    match mint(&keypair, Some("startup self-test")) {
        Ok(_) => eprintln!("signing self-test OK (public key {})", hex(keypair.pk.as_ref())),
        Err(e) => {
            eprintln!("SIGNING SELF-TEST FAILED: {e}");
            eprintln!("Is this seed's public half in ivory-core's PUBLIC_KEYS?");
            std::process::exit(1);
        }
    }

    let addr = format!("0.0.0.0:{port}");
    let server = tiny_http::Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("cannot bind {addr}: {e}");
        std::process::exit(1);
    });
    eprintln!("ivory-fulfil listening on {addr}; hook path /hook/<secret>");

    for mut req in server.incoming_requests() {
        let url = req.url().to_owned();
        let method = req.method().as_str().to_owned();

        // Health check, for the host's uptime probe.
        if method == "GET" && url == "/health" {
            let _ = req.respond(tiny_http::Response::from_string("ok"));
            continue;
        }
        if method != "POST" || url != format!("/hook/{hook_secret}") {
            // Deliberately identical response for wrong-path and wrong-method,
            // so probing cannot distinguish "close" from "nowhere near".
            let _ = req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
            continue;
        }

        let mut body = String::new();
        if req.as_reader().read_to_string(&mut body).is_err() {
            let _ = req.respond(tiny_http::Response::from_string("bad body").with_status_code(400));
            continue;
        }
        let form = parse_form(&body);

        let sale_id = form.get("sale_id").cloned().unwrap_or_default();
        let email = form.get("email").cloned().unwrap_or_default();
        // Gumroad puts custom checkout fields under url_params/variants; the
        // "How should Ivory thank you?" answer is optional by design.
        let display_name = form
            .get("full_name")
            .or_else(|| form.get("purchaser_name"))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        if sale_id.is_empty() || email.is_empty() {
            eprintln!("ping missing sale_id or email; ignoring");
            let _ = req.respond(tiny_http::Response::from_string("missing fields").with_status_code(400));
            continue;
        }
        if !seller_id.is_empty() && form.get("seller_id").map(String::as_str) != Some(seller_id.as_str()) {
            eprintln!("ping seller_id mismatch for sale {sale_id}; ignoring");
            let _ = req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
            continue;
        }

        // Idempotency FIRST: Gumroad retries, and a buyer must never receive two
        // different keys for one purchase.
        if let Some(existing) = ledger_lookup(&ledger, &sale_id) {
            eprintln!("sale {sale_id} already fulfilled; re-sending the original key");
            match send_email(&resend_key, &mail_from, &email, "", &existing) {
                Ok(()) => {
                    let _ = req.respond(tiny_http::Response::from_string("resent"));
                }
                Err(e) => {
                    eprintln!("resend failed for {sale_id}: {e}");
                    // 500 so Gumroad retries later.
                    let _ = req.respond(tiny_http::Response::from_string("mail failed").with_status_code(500));
                }
            }
            continue;
        }

        if !sale_is_genuine(&gumroad_token, &sale_id) {
            eprintln!("sale {sale_id} did not verify against Gumroad; refusing to mint");
            let _ = req.respond(tiny_http::Response::from_string("unverified").with_status_code(403));
            continue;
        }

        let (key, order_id) = match mint(&keypair, display_name.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("MINT FAILED for {sale_id}: {e}");
                let _ = req.respond(tiny_http::Response::from_string("mint failed").with_status_code(500));
                continue;
            }
        };

        // Ledger BEFORE mail: a mail outage must never lose a key a buyer paid
        // for. Anything in the ledger can be re-sent; anything only in an email
        // that failed to send is gone.
        let entry = serde_json::json!({
            "sale_id": sale_id,
            "order": hex(&order_id),
            "email": email,
            "name": display_name,
            "issued_days": issued_days_now(),
            "key": key,
        });
        if let Err(e) = ledger_append(&ledger, &entry) {
            eprintln!("LEDGER WRITE FAILED for {sale_id}: {e} — refusing to send unrecorded key");
            let _ = req.respond(tiny_http::Response::from_string("ledger failed").with_status_code(500));
            continue;
        }

        match send_email(&resend_key, &mail_from, &email, display_name.as_deref().unwrap_or(""), &key) {
            Ok(()) => {
                eprintln!("fulfilled sale {sale_id} -> {email} (order {})", hex(&order_id));
                let _ = req.respond(tiny_http::Response::from_string("ok"));
            }
            Err(e) => {
                // The key is safely in the ledger; 500 asks Gumroad to retry,
                // and the retry will re-send rather than re-mint.
                eprintln!("mail failed for {sale_id} (key IS in the ledger): {e}");
                let _ = req.respond(tiny_http::Response::from_string("mail failed").with_status_code(500));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eyeball the real thing: `cargo test -- --ignored --nocapture print_email`
    #[test]
    #[ignore]
    fn print_email() {
        println!("\n----- BEGIN -----\n{}\n----- END -----", email_body("IVRY-XXXX-XXXX-XXXX-XXXX"));
    }

    /// The download links are the whole reason a buyer keeps this email, and a
    /// silent `format!` mistake would ship a broken URL to every customer.
    #[test]
    fn email_carries_key_and_all_three_downloads() {
        let body = email_body("IVRY-TEST-KEY");
        assert!(body.contains("IVRY-TEST-KEY"), "key missing from email");
        for url in [DOWNLOAD_MACOS, DOWNLOAD_WINDOWS, DOWNLOAD_LINUX] {
            assert!(body.contains(url), "download link missing: {url}");
            assert!(
                url.contains("/releases/latest/download/"),
                "{url} is version-pinned; buyers keep this email for years"
            );
        }
    }

    /// The owner's standing rule for anything sent to a person: no em dashes.
    #[test]
    fn email_has_no_em_dashes() {
        assert!(!email_body("K").contains('\u{2014}'));
    }
}
