//! The second spike: instantiate a plugin, play a note, and prove it made sound.
//!
//!   cargo run -p ivory-host --example playnote -- "Pianoteq"
//!
//! Renders two seconds, reports peak and RMS, and writes the audio next to the
//! binary so it can be listened to. A plugin that instantiates but returns
//! silence is the single most common hosting bug, and it looks exactly like
//! success from every log line except this one.
use ivory_host::{Instance, Module, Note, Setup};

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_else(|| "Pianoteq".into());
    let Some(bundle) = ivory_host::discover()
        .into_iter()
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase().contains(&filter.to_lowercase()))
                .unwrap_or(false)
        })
    else {
        eprintln!("no VST3 matching {filter:?}");
        std::process::exit(1);
    };

    println!("module:   {}", bundle.display());
    let module = Module::open(&bundle).expect("open module");
    println!("vendor:   {}", module.vendor());

    let class = module
        .audio_modules()
        .into_iter()
        .next()
        .expect("no Audio Module Class");
    println!("class:    {}", class.name);

    let setup = Setup { sample_rate: 48_000.0, max_block: 512 };
    let mut inst = Instance::create(&module, &class, setup).expect("instantiate");
    println!("audio out: {:?}", inst.audio_outputs());
    println!("event in:  {:?}", inst.event_inputs());

    let channels = inst.audio_outputs().first().map(|b| b.channels).unwrap_or(0) as usize;
    let mut bufs: Vec<Vec<f32>> = vec![Vec::new(); channels.max(2)];

    // Warm-up: some plugins load samples or presets ASYNCHRONOUSLY after
    // instantiation and render silence until that finishes. A host that
    // measures immediately concludes "broken plugin". Render (and discard)
    // this many seconds before the note, settable so the difference between
    // "needs time" and "never works" is one argument rather than a guess.
    let warmup_s: f64 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(0.0);
    let warm_blocks = (warmup_s * 48_000.0 / 512.0) as usize;
    for _ in 0..warm_blocks {
        inst.process(&[], 512, &mut bufs).expect("warmup");
    }
    if warm_blocks > 0 {
        println!("warmed up {warmup_s}s ({warm_blocks} blocks) before the note");
    }

    // Middle C, mezzo-forte, held for the whole render.
    let blocks = (2.0 * 48_000.0 / 512.0) as usize;
    let mut peak = 0.0f32;
    let mut sumsq = 0.0f64;
    let mut n = 0u64;
    let mut interleaved: Vec<f32> = Vec::new();

    for b in 0..blocks {
        let notes: Vec<Note> = if b == 0 {
            vec![Note { offset: 0, pitch: 60, velocity: 100.0 / 127.0, on: true }]
        } else {
            vec![]
        };
        inst.process(&notes, 512, &mut bufs).expect("process");
        for i in 0..512 {
            for ch in bufs.iter().take(channels) {
                let s = ch[i];
                peak = peak.max(s.abs());
                sumsq += (s as f64) * (s as f64);
                n += 1;
                interleaved.push(s);
            }
        }
    }

    // Write it with the recorder's own writer, bext and all: if the two lanes
    // disagree about anything, this is where it shows up rather than in a demo.
    use ivory_record::wav::{Bext, SampleFormat, WavSpec, WavWriter};
    use ivory_record::take::WallTime;
    let spec = WavSpec { sample_rate: 48_000, channels: channels.max(1) as u16,
                         format: SampleFormat::Int24 };
    let at = WallTime::from_unix(ivory_record::take::unix_now_seconds(), 0);
    let out_path = std::env::temp_dir().join("tangent-playnote.wav");
    let mut w = WavWriter::create(&out_path, spec, &Bext::new(at, spec)).expect("wav");
    w.write_interleaved(&interleaved).expect("write");
    w.finish().expect("finish");

    let rms = (sumsq / n.max(1) as f64).sqrt();
    println!("\nrendered {blocks} blocks, {} samples", n);
    println!("peak: {peak:.6}   rms: {rms:.6}");
    if peak < 1e-6 {
        println!("\nSILENCE. The plugin instantiated and produced nothing.");
        std::process::exit(2);
    }
    println!("\nIT MADE SOUND.  ->  {}", out_path.display());
}
