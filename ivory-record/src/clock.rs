//! The recorder's timebase: how a device timestamp becomes a moment in a take.
//!
//! Every complaint about every tool this feature competes with reduces to sync.
//! OBS users report drift growing "from 300 ms to 700 ms to over a full second"
//! across a 35-minute stream; SeeMusic, the only product that does this whole
//! job, is reported to lose sync on long pieces. This module is where that is
//! won or lost, so it is pure arithmetic with no devices in it and no `Instant`
//! inside it — the caller supplies `now_ns`. That is what makes a 20-minute
//! adversarial take a one-second unit test.
//!
//! # Two roles, and conflating them is the classic failure
//!
//! **The TIMEBASE is the coordinate system**: nanoseconds since an epoch the
//! recorder picks, in `std::time::Instant`'s domain. Every event from every
//! source is converted into it exactly once, at the boundary where it enters
//! Tangent, and nothing downstream ever sees a device timestamp again.
//!
//! **The RATE MASTER is the captured audio sample count.** The exported WAV is
//! N samples long declaring rate `R_nominal`; nothing can change that, because
//! you wrote N samples. So the exported timeline is `t(n) = n / R_nominal` and
//! everything else maps onto *that*, not onto the timebase. See [`Timeline`].
//!
//! # What a shared clock domain does and does not buy you
//!
//! On macOS all four sources live in one domain: `Instant` is `CLOCK_UPTIME_RAW`
//! (std's own source says it is "identical to the result of
//! `mach_absolute_time()` after the appropriate mach timebase conversion"),
//! cpal's CoreAudio `StreamInstant` is `mHostTime`, midir's CoreMIDI stamp
//! derives from `AudioConvertHostTimeToNanos`, and AVFoundation's
//! `CMSampleBuffer` PTS is on `CMClockGetHostTimeClock()`.
//!
//! **That is a shared domain, not a shared origin, and not even a shared
//! scale.** Two traps, both of which an earlier draft of the design walked into:
//!
//! 1. **midir hands you MICROSECONDS**, on every backend — its public contract
//!    says so, and the CoreMIDI backend literally divides the nanos by 1000. The
//!    conversion is `× 1000`, which is what [`SourceClock::new`]'s `scale_ns`
//!    is for. `MFSampleExtension_DeviceTimestamp` on Windows is 100 ns units,
//!    i.e. `× 100`. Getting this wrong is a factor-of-1000 error that looks like
//!    a dead clock rather than a wrong one.
//! 2. **Every source has a latency and no clock arithmetic removes it.** A
//!    running minimum kills jitter; by construction it cannot kill a constant.
//!    See [`SourceClock::set_latency`].
//!
//! So **no source skips anchoring**, including macOS MIDI. A shared domain means
//! the scale is known and the offset is small, which is a better starting
//! position than the other platforms and is not the same thing as being solved.

/// Nanoseconds in the recorder's timebase.
///
/// Signed, because events legitimately precede `T0`: the pre-roll ring holds
/// audio and frames from before Record was pressed, and the always-on MIDI tap
/// has been running since the port opened. Clamping those to zero at capture
/// time would throw away exactly the messages the tick-0 controller snapshot
/// needs.
pub type Nanos = i64;

/// One second, in nanoseconds.
pub const NS_PER_SEC: f64 = 1_000_000_000.0;

/// midir's callback stamp is microseconds on every backend.
///
/// Documented at `midir/src/common.rs:101-104` and `src/lib.rs:42-44`; the
/// CoreMIDI backend is `AudioConvertHostTimeToNanos(t) as u64 / 1000`
/// (`src/backend/coremidi/mod.rs:104-105`). Named rather than written as a
/// literal `1000.0` at the call site, because a bare 1000 in this file could
/// plausibly be several other things.
pub const MIDIR_SCALE_NS: f64 = 1_000.0;

/// `MFSampleExtension_DeviceTimestamp` is "always expressed in 100ns units".
pub const MF_SCALE_NS: f64 = 100.0;

/// Maps one source's device timestamps into the timebase.
///
/// The model is affine with a latency correction:
///
/// ```text
/// T = scale_ns * stamp + offset - latency
/// ```
///
/// `scale_ns` is known from the source's documented unit. `offset` is estimated
/// from observations. `latency` is supplied by whoever knows it — the OS, a
/// calibration, or nobody (in which case it is zero and the take says so).
#[derive(Debug, Clone)]
pub struct SourceClock {
    scale_ns: f64,
    /// Smallest offset seen so far. `None` until the first observation.
    offset: Option<f64>,
    latency: Nanos,
    /// Observations are still improving the estimate until this timebase
    /// instant. After it, `offset` is frozen.
    settle_deadline: Option<Nanos>,
    settle_ns: Nanos,
    observations: u64,
    /// The largest and smallest offsets ever seen, for the take's sync report.
    /// The spread is this source's delivery jitter and is worth showing a user
    /// who reports a problem.
    offset_min: f64,
    offset_max: f64,
}

impl SourceClock {
    /// `scale_ns` converts one unit of this source's stamp to nanoseconds
    /// ([`MIDIR_SCALE_NS`], [`MF_SCALE_NS`], or `1.0` for a source already in
    /// nanoseconds). `settle_ns` is how long observations keep improving the
    /// offset estimate — two seconds is the recommendation.
    pub fn new(scale_ns: f64, settle_ns: Nanos) -> Self {
        Self {
            scale_ns,
            offset: None,
            latency: 0,
            settle_deadline: None,
            settle_ns,
            observations: 0,
            offset_min: f64::INFINITY,
            offset_max: f64::NEG_INFINITY,
        }
    }

    /// Record one `(device stamp, timebase now)` pair.
    ///
    /// **The estimator is a running MINIMUM, not an average and not the first
    /// sample.** Delivery latency only ever *adds* to the observed offset — a
    /// callback can be late, never early — so the smallest observed offset is
    /// the least-delayed observation and therefore the best estimate of the
    /// true one. This is the standard NTP/PTP treatment of asymmetric delay and
    /// it converges to the jitter floor.
    ///
    /// Anchoring on the first callback instead, which is the obvious thing to
    /// do, bakes in one full callback of scheduling latency permanently.
    pub fn observe(&mut self, stamp: u64, now_ns: Nanos) {
        let offset = now_ns as f64 - self.scale_ns * stamp as f64;

        self.observations += 1;
        self.offset_min = self.offset_min.min(offset);
        self.offset_max = self.offset_max.max(offset);

        let deadline = *self.settle_deadline.get_or_insert(now_ns + self.settle_ns);

        match self.offset {
            // Always take the first one: an estimate that is merely late beats
            // no estimate at all, and the take may be shorter than settle_ns.
            None => self.offset = Some(offset),
            Some(current) if now_ns <= deadline && offset < current => {
                self.offset = Some(offset);
            }
            Some(_) => {}
        }
    }

    /// Subtract a known device latency from every converted timestamp.
    ///
    /// This is the term a shared clock domain does **not** give you. A UVC
    /// webcam's sensor → ISP → USB → host path is 20-150 ms and AVFoundation
    /// exposes no way to query it; cpal's CoreAudio input does subtract
    /// `kAudioDevicePropertyLatency + kAudioDevicePropertySafetyOffset`, but
    /// each is `.unwrap_or(0)`, so a device that reports nothing silently gets
    /// zero.
    ///
    /// The camera term is the one that matters here, and this feature makes it
    /// maximally visible: the composite puts the camera and the MIDI-driven key
    /// display in the *same frame*, so uncompensated, the keys light up before
    /// the filmed fingers land. That is a far lower detection threshold than
    /// lip-sync, because the eye compares two things inside one image.
    pub fn set_latency(&mut self, latency_ns: Nanos) {
        self.latency = latency_ns;
    }

    pub fn latency(&self) -> Nanos {
        self.latency
    }

    pub fn observations(&self) -> u64 {
        self.observations
    }

    /// Peak-to-peak spread of observed offsets: this source's delivery jitter.
    /// `None` before the first observation.
    pub fn jitter_ns(&self) -> Option<Nanos> {
        (self.observations > 0).then(|| (self.offset_max - self.offset_min) as Nanos)
    }

    /// Convert a device stamp into the timebase. `None` before the first
    /// [`observe`](Self::observe).
    pub fn to_timebase(&self, stamp: u64) -> Option<Nanos> {
        let offset = self.offset?;
        Some((self.scale_ns * stamp as f64 + offset) as Nanos - self.latency)
    }
}

/// Incremental least-squares fit of `T = alpha * n + beta`, where `n` is an
/// audio sample index and `T` is a timebase instant.
///
/// `alpha` is the true nanoseconds per sample measured against the host clock,
/// so `1e9 / alpha` is the device's *true* rate as opposed to its nominal one.
/// That difference is the entire drift problem: at a typical ±50 ppm crystal it
/// is **60 ms at the end of a 20-minute take**, which is monotone drift rather
/// than jitter and therefore exactly what a listener notices.
///
/// Sums are accumulated about the first observation rather than about zero.
/// That is not tidiness: sample indices reach ~5.8e7 in twenty minutes and
/// timebase values are ~1.2e12, so an uncentred `sxx`/`sxy` would be squaring
/// numbers that large in `f64` and losing the low bits that carry the slope.
#[derive(Debug, Clone, Default)]
pub struct RateFit {
    origin: Option<(f64, f64)>,
    count: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
}

impl RateFit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that the audio frame with index `sample` was captured at `t`.
    pub fn push(&mut self, sample: u64, t: Nanos) {
        let (nx, ny) = *self.origin.get_or_insert((sample as f64, t as f64));
        let x = sample as f64 - nx;
        let y = t as f64 - ny;
        self.count += 1.0;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.sxy += x * y;
    }

    pub fn observations(&self) -> u64 {
        self.count as u64
    }

    /// Nanoseconds per sample. `None` until two observations at distinct sample
    /// indices exist — one point has no slope, and two identical points have a
    /// zero denominator rather than an infinite slope.
    pub fn alpha(&self) -> Option<f64> {
        let denom = self.count * self.sxx - self.sx * self.sx;
        (self.count >= 2.0 && denom.abs() > f64::EPSILON)
            .then(|| (self.count * self.sxy - self.sx * self.sy) / denom)
    }

    /// The timebase instant of sample 0.
    pub fn beta(&self) -> Option<f64> {
        let (nx, ny) = self.origin?;
        let alpha = self.alpha()?;
        let intercept = (self.sy - alpha * self.sx) / self.count;
        Some(ny + intercept - alpha * nx)
    }

    /// The device's measured rate in Hz.
    pub fn true_rate(&self) -> Option<f64> {
        self.alpha().map(|a| NS_PER_SEC / a)
    }

    /// `R_true / R_nominal - 1`, the fractional rate error. Multiply by 1e6 for
    /// ppm. This is the `epsilon` the whole drift correction turns on.
    pub fn epsilon(&self, nominal_rate: f64) -> Option<f64> {
        self.true_rate().map(|r| r / nominal_rate - 1.0)
    }
}

/// Everything needed to place an instant into the exported files.
///
/// Built once at Stop, when the fit is complete, and then used to stamp MIDI
/// ticks and video PTS. One `Timeline` serves all three deliverables, which is
/// what makes "MIDI tick 0, WAV sample 0 and video frame 0 are the same
/// instant" a fact rather than an aspiration.
#[derive(Debug, Clone, Copy)]
pub struct Timeline {
    t0: Nanos,
    t1: Nanos,
    nominal_rate: f64,
    /// Timebase instant of audio sample 0, from the fit. Falls back to `t0`
    /// when there was no audio device to fit against.
    beta: f64,
    epsilon: f64,
    synthetic: bool,
}

impl Timeline {
    /// The normal case: an audio input device was the rate master.
    ///
    /// `t0` and `t1` are the take's start and stop instants in the timebase.
    /// If the fit is too short to yield a slope, this degrades to
    /// [`synthetic`](Self::synthetic) rather than inventing one.
    pub fn from_fit(t0: Nanos, t1: Nanos, nominal_rate: f64, fit: &RateFit) -> Self {
        match (fit.beta(), fit.epsilon(nominal_rate)) {
            (Some(beta), Some(epsilon)) => Self {
                t0,
                t1,
                nominal_rate,
                beta,
                epsilon,
                synthetic: false,
            },
            _ => Self::synthetic(t0, t1, nominal_rate),
        }
    }

    /// No audio device is being sampled, so there is no crystal to drift
    /// against: plugin-only mode, or a fit too short to be meaningful.
    ///
    /// `epsilon` is exactly zero **by definition, not by assumption**, and the
    /// take's sync report must say `"clock": "synthetic"` rather than reporting
    /// a fit that was never made. Quietly reporting 0 ppm from a real fit and
    /// 0 ppm from no fit at all is how a broken pipeline looks healthy.
    pub fn synthetic(t0: Nanos, t1: Nanos, nominal_rate: f64) -> Self {
        Self {
            t0,
            t1,
            nominal_rate,
            beta: t0 as f64,
            epsilon: 0.0,
            synthetic: true,
        }
    }

    pub fn t0(&self) -> Nanos {
        self.t0
    }

    pub fn t1(&self) -> Nanos {
        self.t1
    }

    pub fn is_synthetic(&self) -> bool {
        self.synthetic
    }

    pub fn epsilon(&self) -> f64 {
        self.epsilon
    }

    /// Parts per million of rate error, for the take's sync report.
    pub fn epsilon_ppm(&self) -> f64 {
        self.epsilon * 1e6
    }

    pub fn nominal_rate(&self) -> f64 {
        self.nominal_rate
    }

    /// Where a timebase instant lands in the exported files, in seconds from
    /// the start of the take.
    ///
    /// The derivation, because the one multiply at the end is the whole drift
    /// correction and it is worth being able to check:
    ///
    /// ```text
    /// n(T)      = (T - beta) / alpha                  fractional sample index
    /// file_t(T) = n(T) / R_nominal
    ///           = (T - beta) / (alpha * R_nominal)
    ///           = (T - beta) * (1 + epsilon) / 1e9    since alpha*R_nom = 1e9/(1+eps)
    /// ```
    ///
    /// Then subtract `T0`'s own position, so sample 0 is second 0.
    ///
    /// Skipping `(1 + epsilon)` costs 50e-6 * 1200 s = 60 ms at the end of a
    /// twenty-minute take. That is well inside the range a listener notices and
    /// it is a *drift*, which is worse than an offset of the same size because
    /// it cannot be dialled out.
    pub fn file_seconds(&self, t: Nanos) -> f64 {
        let at = |x: Nanos| (x as f64 - self.beta) * (1.0 + self.epsilon) / NS_PER_SEC;
        at(t) - at(self.t0)
    }

    /// The same instant as a fractional index into the exported WAV.
    pub fn file_sample(&self, t: Nanos) -> f64 {
        self.file_seconds(t) * self.nominal_rate
    }

    /// The take's duration in seconds, on the exported timeline.
    pub fn duration_seconds(&self) -> f64 {
        self.file_seconds(self.t1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_minimum_beats_the_first_observation() {
        // Truth: stamp is microseconds, offset is 5_000_000 ns.
        // Delivery is late by a varying amount, and the FIRST callback is the
        // latest of all — which is the common case, because the first one pays
        // for page faults and lazy initialisation.
        let mut c = SourceClock::new(MIDIR_SCALE_NS, 2_000_000_000);
        let truth = 5_000_000i64;
        for (i, late) in [900_000i64, 300_000, 50_000, 120_000].iter().enumerate() {
            let stamp = (i as u64 + 1) * 1_000; // µs
            let true_t = MIDIR_SCALE_NS as i64 * stamp as i64 + truth;
            c.observe(stamp, true_t + late);
        }
        let err = (c.to_timebase(1_000).unwrap() - (1_000_000 + truth)).abs();
        assert!(
            err <= 50_000,
            "running minimum should converge to the least-delayed observation, \
             leaving only its 50 µs lateness; got {err} ns of error"
        );
    }

    #[test]
    fn the_microsecond_scale_is_not_the_identity() {
        // The bug this test exists for: treating midir's stamp as nanoseconds.
        // One second of MIDI would then land one millisecond into the take.
        let mut c = SourceClock::new(MIDIR_SCALE_NS, 1_000_000_000);
        c.observe(0, 0);
        assert_eq!(
            c.to_timebase(1_000_000),
            Some(1_000_000_000),
            "1e6 microseconds is one second"
        );
    }

    #[test]
    fn latency_is_subtracted_and_a_minimum_filter_cannot_replace_it() {
        let mut c = SourceClock::new(1.0, 1_000_000_000);
        // A source with a PERFECTLY consistent 40 ms pipeline delay. No jitter
        // at all, so the running minimum has nothing to work with.
        for i in 0..50u64 {
            let stamp = i * 10_000_000;
            c.observe(stamp, stamp as i64 + 40_000_000);
        }
        assert_eq!(
            c.to_timebase(0),
            Some(40_000_000),
            "with no jitter the estimator can only see the delayed truth"
        );
        c.set_latency(40_000_000);
        assert_eq!(
            c.to_timebase(0),
            Some(0),
            "only an explicit latency term removes a constant"
        );
    }

    #[test]
    fn a_fit_recovers_a_50ppm_error() {
        let nominal = 48_000.0;
        let truth = 48_002.4; // +50 ppm
        let mut fit = RateFit::new();
        for k in 0..2_000u64 {
            let sample = k * 512;
            let t = (sample as f64 / truth * NS_PER_SEC) as Nanos;
            fit.push(sample, t);
        }
        let ppm = fit.epsilon(nominal).unwrap() * 1e6;
        assert!(
            (ppm - 50.0).abs() < 0.5,
            "expected ~50 ppm, fitted {ppm:.3} ppm"
        );
    }

    /// The number quoted in the design doc, checked rather than asserted.
    #[test]
    fn twenty_minutes_of_uncorrected_drift_is_sixty_milliseconds() {
        let nominal = 48_000.0;
        let truth = 48_002.4;
        let mut fit = RateFit::new();
        for k in 0..20_000u64 {
            let sample = k * 512;
            fit.push(sample, (sample as f64 / truth * NS_PER_SEC) as Nanos);
        }
        let t0 = 0;
        let t1 = 1_200 * 1_000_000_000;
        let corrected = Timeline::from_fit(t0, t1, nominal, &fit);
        let uncorrected = Timeline::synthetic(t0, t1, nominal);

        let delta_ms =
            (corrected.file_seconds(t1) - uncorrected.file_seconds(t1)) * 1_000.0;
        assert!(
            (delta_ms - 60.0).abs() < 1.0,
            "the correction should be worth ~60 ms over 20 minutes, got {delta_ms:.2} ms"
        );
    }

    #[test]
    fn t0_is_second_zero_whatever_the_epoch() {
        let mut fit = RateFit::new();
        for k in 0..1_000u64 {
            let sample = k * 512;
            fit.push(sample, 987_654_321 + (sample as f64 / 48_000.0 * NS_PER_SEC) as Nanos);
        }
        let t0 = 987_654_321 + 3_000_000_000;
        let tl = Timeline::from_fit(t0, t0 + 1_000_000_000, 48_000.0, &fit);
        assert!(tl.file_seconds(t0).abs() < 1e-9, "T0 must be exactly zero");
        assert!((tl.duration_seconds() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_fit_too_short_to_have_a_slope_degrades_to_synthetic() {
        let fit = RateFit::new();
        let tl = Timeline::from_fit(0, 1_000, 48_000.0, &fit);
        assert!(
            tl.is_synthetic(),
            "no observations must not silently report a 0 ppm measurement"
        );
        assert_eq!(tl.epsilon(), 0.0);
    }

    #[test]
    fn events_before_t0_are_negative_rather_than_clamped() {
        let tl = Timeline::synthetic(1_000_000_000, 2_000_000_000, 48_000.0);
        assert!(
            tl.file_seconds(500_000_000) < 0.0,
            "pre-roll must stay negative here; clamping is the SMF writer's job \
             and doing it twice loses the controller snapshot"
        );
    }
}
