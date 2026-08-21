//! The transport: one owner for the playhead, in seconds.
//!
//! # Why the HOST owns it, and the engine only answers
//!
//! The engine is dropped and rebuilt whenever the audio path changes — a
//! buffer size, a sample rate, an audio system, the band closing — and a
//! position that lived only in `Shared` silently returned to 0:00 mid-session
//! with nothing host-side to restore it from. So the playhead's home is here,
//! in SECONDS, because seconds survive a device that changes rate; the engine
//! holds the sample-exact copy and this struct reconciles the two.
//!
//! # The generation, and why a locate is an event
//!
//! `set_track_playing` was a LEVEL re-asserted sixty times a second, and the
//! audio thread found its edges — which made a locate inexpressible: a bare
//! "seek to N" store could not be told from the same value re-asserted. The
//! locate is now a GENERATION-stamped command (`Engine::set_transport`), the
//! callback acks the generation it has applied, and the level (`set_rolling`)
//! is derived and safe to re-assert because nothing keys off its edges.
//!
//! The generation is this struct's monotonic counter, starting at 1 and never
//! reset — deliberately, so a freshly built engine (whose applied generation
//! is 0) never matches and is re-located by the first push. That is the whole
//! of engine-rebuild recovery.
//!
//! # What the readout shows
//!
//! [`Transport::position_s`] promises exactly one live authority at a time:
//! the REQUESTED position until the engine acks it, the engine's sample clock
//! after, and the last known truth — never a surprise 0:00 — when there is no
//! engine at all. The gap it closes is small and visible: without it, a click
//! on the waveform drew the playhead at the old position for a frame (~16 ms)
//! and then snapped, which is the gesture the whole feature is judged by.

use crate::instrument::Engine;

/// The playhead, the play switch, and the bookkeeping that keeps the engine's
/// copy honest. One per app.
pub struct Transport {
    /// The playhead in seconds from 0:00. The one owner.
    pos_s: f64,
    /// Monotonic locate counter. Starts at 1 so a fresh engine (ack 0) always
    /// re-locates on the first push.
    generation: u64,
    /// Whether PLAY is held down — the audition, distinct from a take rolling.
    /// A take is the session's business; this is the green button's.
    playing: bool,
    /// The session's rolling level as of the last push, for the falling edge
    /// that returns the playhead to 0:00.
    was_rolling: bool,
}

impl Transport {
    pub fn new() -> Self {
        Self {
            pos_s: 0.0,
            generation: 1,
            playing: false,
            was_rolling: false,
        }
    }

    /// Put the playhead somewhere, now. Legal while rolling — that is the
    /// point of the generation — and legal with no engine, where it simply
    /// waits to be published.
    pub fn locate(&mut self, seconds: f64) {
        self.pos_s = seconds.max(0.0);
        self.generation += 1;
    }

    /// The green button. Starts the audition from the playhead; pressed while
    /// auditioning, stops and returns to 0:00 — the owner's rule, not a pause.
    pub fn toggle_play(&mut self) {
        if self.playing {
            self.playing = false;
            self.locate(0.0);
        } else {
            self.playing = true;
        }
    }

    /// Whether the audition is running.
    pub fn playing(&self) -> bool {
        self.playing
    }

    /// The playhead, for the readout and the waveform.
    ///
    /// One authority at a time: the pending locate until the engine has
    /// applied it, the sample clock after, the remembered position with no
    /// engine. `rate` is the engine's own, asked at the call site so this
    /// module never caches a number the device owns.
    pub fn position_s(&self, engine: Option<&Engine>, rate: f64) -> f64 {
        match engine {
            Some(e) if e.transport_acked(self.generation) && rate > 0.0 => {
                e.transport_position() as f64 / rate
            }
            _ => self.pos_s,
        }
    }

    /// Reconcile once a frame, AFTER the session has ticked.
    ///
    /// After, because the rolling level is derived from the session's state
    /// and publishing it a frame early is how the take's first block used to
    /// start from the old position. The order inside matters and is the safe
    /// one twice over: the locate is published BEFORE the level rises (a
    /// callback that sees the level up is guaranteed a position that is not
    /// stale), and the level falls BEFORE the return-to-zero locate (so no
    /// block rolls at zero on the way down).
    pub fn push(&mut self, engine: Option<&Engine>, session_rolling: bool) {
        let rolling = session_rolling || self.playing;
        let Some(e) = engine else {
            self.was_rolling = rolling;
            return;
        };
        let rate = f64::from(e.output().sample_rate).max(1.0);
        // While rolling and acked, the engine's clock is the truth — carried
        // back into seconds continuously, so an engine that dies mid-roll is
        // survived by the position it had reached, not by where it started.
        if rolling && e.transport_acked(self.generation) {
            self.pos_s = e.transport_position() as f64 / rate;
        }
        // The falling edge: stop returns to 0:00. Both stops — a take ending
        // and the audition ending — arrive here, which is what makes the rule
        // one rule.
        if self.was_rolling && !rolling {
            self.locate(0.0);
        }
        self.was_rolling = rolling;
        if !e.transport_acked(self.generation) {
            let frames = (self.pos_s.max(0.0) * rate) as u64;
            e.set_transport(self.generation, frames);
        }
        e.set_rolling(rolling);
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}
