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
    /// The SESSION's rolling level as of the last push, for the falling edge
    /// that returns the playhead to 0:00 when a take ends. The audition's own
    /// falling edge is a PAUSE and deliberately not part of this.
    session_was_rolling: bool,
}

impl Transport {
    pub fn new() -> Self {
        Self {
            pos_s: 0.0,
            generation: 1,
            playing: false,
            session_was_rolling: false,
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
    /// auditioning, PAUSES — the playhead stays put, and the next press picks
    /// up from there. Going home to 0:00 is the stop button's job, which is
    /// what makes the pair a tape transport rather than two spellings of stop.
    pub fn toggle_play(&mut self) {
        self.playing = !self.playing;
    }

    /// The square. Ends the audition if one is running and sends the playhead
    /// home — from a pause as well, because "stop" from anywhere means 0:00.
    pub fn stop(&mut self) {
        self.playing = false;
        self.locate(0.0);
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
        // The falling edge of a TAKE returns the playhead to 0:00. Only a
        // take's: the audition's own falling edge is a pause and keeps its
        // place — see `toggle_play`. And not even a take's when the audition
        // is still running through it, or stopping a take somebody recorded
        // over a rolling audition would yank the music back to the top.
        //
        // Before the engine is even unwrapped: the playhead is this struct's,
        // and a take that ends in the gap while an engine is being rebuilt
        // still ended.
        if self.session_was_rolling && !session_rolling && !self.playing {
            self.locate(0.0);
        }
        self.session_was_rolling = session_rolling;
        let Some(e) = engine else {
            return;
        };
        let rate = f64::from(e.output().sample_rate).max(1.0);
        // While rolling and acked, the engine's clock is the truth — carried
        // back into seconds continuously, so an engine that dies mid-roll is
        // survived by the position it had reached, not by where it started.
        if rolling && e.transport_acked(self.generation) {
            self.pos_s = e.transport_position() as f64 / rate;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// **Play pauses in place; only stop goes home.** The second press of
    /// play used to locate to 0:00, which made it a second stop button — a
    /// transport where you cannot stop to listen and carry on from there.
    #[test]
    fn play_pauses_in_place_and_stop_goes_home() {
        let mut t = Transport::new();
        t.locate(83.0);
        t.toggle_play();
        assert!(t.playing());
        t.push(None, false);
        t.toggle_play();
        assert!(!t.playing(), "the second press did not pause");
        t.push(None, false);
        assert!(
            (t.position_s(None, 0.0) - 83.0).abs() < 1e-9,
            "pausing moved the playhead to {}",
            t.position_s(None, 0.0)
        );
        t.stop();
        assert!(
            t.position_s(None, 0.0).abs() < 1e-9,
            "stop did not send the playhead home"
        );
    }

    /// A take that ends still returns to 0:00 — that rule kept its half.
    #[test]
    fn a_take_ending_still_returns_to_zero() {
        let mut t = Transport::new();
        t.locate(10.0);
        t.push(None, true);
        t.push(None, false);
        assert!(
            t.position_s(None, 0.0).abs() < 1e-9,
            "a finished take left the playhead at {}",
            t.position_s(None, 0.0)
        );
    }

    /// And a take ending UNDER a running audition does not yank it home.
    #[test]
    fn a_take_ending_under_the_audition_leaves_it_rolling() {
        let mut t = Transport::new();
        t.locate(45.0);
        t.toggle_play();
        t.push(None, true);
        t.push(None, false);
        assert!(t.playing());
        assert!(
            (t.position_s(None, 0.0) - 45.0).abs() < 1e-9,
            "the take's end moved a rolling audition to {}",
            t.position_s(None, 0.0)
        );
    }
}
