//! The spike: does a real VST3 load, and what does it say it is?
//!
//!   cargo run -p ivory-host --example scanvst
//!   cargo run -p ivory-host --example scanvst -- Pianoteq
//!
//! Filters by substring when given an argument. Prints one line per module and
//! one indented line per exported class.
fn main() {
    let filter = std::env::args().nth(1);
    let bundles = ivory_host::discover();
    println!("{} VST3 bundles found\n", bundles.len());

    let (mut ok, mut failed, mut instruments) = (0, 0, 0);
    for bundle in &bundles {
        let name = bundle.file_name().unwrap_or_default().to_string_lossy();
        if let Some(f) = &filter {
            if !name.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }
        match ivory_host::Module::open(bundle) {
            Ok(m) => {
                ok += 1;
                let classes = m.classes();
                let audio = classes.iter().filter(|c| c.is_audio_module()).count();
                instruments += audio;
                println!("{name}  [{}]", m.vendor());
                for c in &classes {
                    println!("    {:<34} {}", c.name, c.category);
                }
            }
            Err(e) => {
                failed += 1;
                println!("{name}  FAILED: {e}");
            }
        }
    }
    println!("\nloaded {ok}, failed {failed}, audio-module classes {instruments}");
}
