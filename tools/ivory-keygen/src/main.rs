//! Ivory supporter-key minting tool. Runs on the owner's machine only.
//!
//! This crate is outside the Ivory workspace on purpose — see Cargo.toml. It
//! links `ivory-core`'s canonical payload/encoder so a minted key is encoded by
//! exactly the code that will verify it.
//!
//! Usage:
//!   ivory-keygen genesis                       # once: create the signing keys
//!   ivory-keygen mint --name "Ada L"           # per sale
//!   ivory-keygen mint --name "Ada L" --key 2   # mint with the successor key
//!   ivory-keygen verify <file>                 # check a key you already sent
//!
//! Seeds live in ~/.ivory-signing/ (0600), which is OUTSIDE the repo — the repo
//! is inside Dropbox, so anything under it is uploaded to a third party
//! regardless of .gitignore. Back that directory up somewhere offline: losing
//! it means you can never mint again; leaking it means anyone can.

use ed25519_compact::{KeyPair, Seed, Signature};
use ivory_core::license::{encode_key, verify_key, License, TIER_SUPPORTER};
use std::io::Write;
use std::path::{Path, PathBuf};

fn seed_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".ivory-signing")
}

fn seed_path(index: u8) -> PathBuf {
    seed_dir().join(format!("k{index}.seed"))
}

fn ledger_path() -> PathBuf {
    seed_dir().join("ledger.jsonl")
}

/// Days since 2020-01-01, computed from the system clock. Display only.
fn issued_days_now() -> u16 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 2020-01-01T00:00:00Z
    const EPOCH_2020: u64 = 1_577_836_800;
    ((secs.saturating_sub(EPOCH_2020)) / 86_400).min(u16::MAX as u64) as u16
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn load_keypair(index: u8) -> KeyPair {
    let p = seed_path(index);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|_| {
        eprintln!("no signing seed at {}", p.display());
        eprintln!("run `ivory-keygen genesis` first (once, ever).");
        std::process::exit(2);
    });
    let raw = unhex(&text).unwrap_or_else(|| {
        eprintln!("seed file {} is not hex", p.display());
        std::process::exit(2);
    });
    let seed = Seed::from_slice(&raw).unwrap_or_else(|_| {
        eprintln!("seed file {} is the wrong length", p.display());
        std::process::exit(2);
    });
    KeyPair::from_seed(seed)
}

fn write_private(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("create seed dir");
    std::fs::write(path, contents).expect("write seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

fn genesis() {
    // Two keys at genesis: k1 signs today, k2 is a cold successor. BOTH public
    // halves ship in the app's ring from v1, so if k1 ever leaks you switch to
    // k2 without invalidating a single license already in the wild.
    for i in 1..=2u8 {
        if seed_path(i).exists() {
            eprintln!("refusing to overwrite {} — genesis runs ONCE.", seed_path(i).display());
            std::process::exit(2);
        }
    }
    println!("Generating two Ed25519 signing keys...\n");
    let mut pubs = Vec::new();
    for i in 1..=2u8 {
        let kp = KeyPair::generate();
        let seed_hex = hex(kp.sk.seed().as_ref());
        write_private(&seed_path(i), &seed_hex);
        pubs.push(kp.pk.as_ref().to_vec());
        println!("  k{i} private seed -> {}  (0600)", seed_path(i).display());
    }
    println!("\nPaste this into ivory-core/src/license.rs, replacing PUBLIC_KEYS:\n");
    println!("pub const PUBLIC_KEYS: &[[u8; 32]] = &[");
    for (i, pk) in pubs.iter().enumerate() {
        let body: Vec<String> = pk.iter().map(|b| format!("0x{b:02x}")).collect();
        println!("    // k{}", i + 1);
        println!("    [");
        for chunk in body.chunks(8) {
            println!("        {},", chunk.join(", "));
        }
        println!("    ],");
    }
    println!("];\n");
    println!("BACK UP {} OFFLINE NOW.", seed_dir().display());
    println!("Lose it and you can never mint another key; leak it and anyone can.");
}

fn mint(name: Option<String>, key_index: u8, max_major: u8, order_note: Option<String>) {
    let kp = load_keypair(key_index);

    // 40 bits of order id, derived from the OS RNG via a throwaway keypair seed
    // (this tool already depends on `random`; no extra dependency for this).
    let entropy = KeyPair::generate();
    let mut order_id = [0u8; 5];
    order_id.copy_from_slice(&entropy.pk.as_ref()[..5]);

    let license = License {
        tier: TIER_SUPPORTER,
        issued_days: issued_days_now(),
        max_major,
        order_id,
        name: name.clone(),
    };
    let payload = license.payload_bytes();
    let sig: Signature = kp.sk.sign(&payload, None);
    let mut sig64 = [0u8; 64];
    sig64.copy_from_slice(sig.as_ref());
    let key = encode_key(&payload, &sig64);

    // Prove it before it leaves the building. A key that does not verify with
    // the app's own code must never reach a customer.
    match verify_key(&key) {
        Ok(v) if v == license => {}
        Ok(_) => {
            eprintln!("INTERNAL ERROR: minted key round-tripped to a different license");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("INTERNAL ERROR: minted key does not verify ({e:?}).");
            eprintln!("Is PUBLIC_KEYS in ivory-core/src/license.rs still the placeholder?");
            eprintln!("Run `ivory-keygen genesis` and paste the output in, then rebuild.");
            std::process::exit(1);
        }
    }

    // Private ledger, so a leaked key can be traced back to a sale. Stays with
    // the seeds, outside the repo.
    let line = format!(
        "{{\"order\":\"{}\",\"issued_days\":{},\"tier\":{},\"key_index\":{},\"name\":{},\"note\":{}}}\n",
        hex(&order_id),
        license.issued_days,
        license.tier,
        key_index,
        serde_json_string(name.as_deref()),
        serde_json_string(order_note.as_deref()),
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path())
    {
        let _ = f.write_all(line.as_bytes());
    }

    println!("{key}");
    eprintln!("\n(order {} logged to {})", hex(&order_id), ledger_path().display());
}

/// Minimal JSON string escaper — avoids pulling serde into this tool.
fn serde_json_string(s: Option<&str>) -> String {
    match s {
        None => "null".to_owned(),
        Some(s) => {
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
    }
}

fn verify_file(path: &str) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(2);
    });
    match verify_key(&text) {
        Ok(l) => {
            println!("VALID");
            println!("  tier        {}", l.tier);
            println!("  issued_days {}", l.issued_days);
            println!("  max_major   {}", l.max_major);
            println!("  order       {}", hex(&l.order_id));
            println!("  name        {}", l.name.as_deref().unwrap_or("(none)"));
        }
        Err(e) => {
            println!("INVALID: {:?} — {}", e, e.message());
            std::process::exit(1);
        }
    }
}

fn usage() -> ! {
    eprintln!("{}", USAGE);
    std::process::exit(2)
}

const USAGE: &str = "\
ivory-keygen — mint Ivory supporter keys (owner tool, never shipped)

  genesis                            create the two signing keys (run once)
  mint [--name NAME] [--key 1|2]     mint a supporter key; prints it on stdout
       [--max-major N] [--note TEXT]
  verify FILE                        verify a key file

Seeds and the sale ledger live in ~/.ivory-signing/ — back it up offline.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("genesis") => genesis(),
        Some("mint") => {
            let (mut name, mut key_index, mut max_major, mut note) = (None, 1u8, 0u8, None);
            let mut i = 1;
            while i < args.len() {
                let need = |i: usize| -> String {
                    args.get(i + 1).cloned().unwrap_or_else(|| usage())
                };
                match args[i].as_str() {
                    "--name" => { name = Some(need(i)); i += 2; }
                    "--note" => { note = Some(need(i)); i += 2; }
                    "--key" => { key_index = need(i).parse().unwrap_or_else(|_| usage()); i += 2; }
                    "--max-major" => { max_major = need(i).parse().unwrap_or_else(|_| usage()); i += 2; }
                    _ => usage(),
                }
            }
            if !(1..=2).contains(&key_index) {
                eprintln!("--key must be 1 or 2");
                std::process::exit(2);
            }
            mint(name, key_index, max_major, note);
        }
        Some("verify") => verify_file(args.get(1).unwrap_or_else(|| usage())),
        _ => usage(),
    }
}
