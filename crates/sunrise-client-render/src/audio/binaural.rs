//! Directional Audio (vanilla's `directionalAudio` option): each positioned mono sound is
//! rendered for two ears from a spherical head, so headphones place it around the player,
//! behind and above too, instead of only left and right.
//!
//! The head is a rigid sphere and every cue comes from its geometry, not from measured ear
//! recordings:
//! - the interaural time difference, the far ear hearing the sound later by the extra path
//!   around the sphere (Woodworth's formula), through a fractional delay;
//! - the head's shadow, a one-pole, one-zero shelf per ear that lifts the highs facing the
//!   sound and cuts them on the far side (the sphere's diffraction, after Brown and Duda's
//!   structural model);
//! - a single pinna reflection whose delay shortens as the sound rises, moving a notch with
//!   elevation, and a gentle high cut for sounds behind, which the outer ear shadows.
//!
//! Every parameter glides across the buffer from where it was, so turning the head or a moving
//! source never clicks.

/// The history of the sound kept for the delays, in frames: a power of two.
pub(super) const HISTORY: usize = 256;
/// Frames every ear is delayed by on top of its cue, so the interpolation always has the two
/// newer samples it reads.
const LOOKAHEAD: f32 = 2.0;
/// The head's radius, in metres, and the speed of sound, in metres a second.
const HEAD_RADIUS: f32 = 0.0875;
const SPEED_OF_SOUND: f32 = 343.0;
/// The head shadow's high-frequency gain at its deepest, and the angle from the ear (degrees)
/// where it is deepest: just short of the far side, where the waves around both sides meet again.
const SHADOW_DEEPEST: f32 = 0.1;
const SHADOW_ANGLE: f32 = 150.0;
/// The pinna reflection's gain, and its delay for a sound straight above and straight below,
/// in seconds: a notch near 8.3 kHz overhead sliding to 5 kHz underfoot.
const PINNA_GAIN: f32 = 0.35;
const PINNA_ABOVE: f32 = 60e-6;
const PINNA_BELOW: f32 = 100e-6;
/// How much of the highs a sound straight behind loses, and where they start.
const REAR_AMOUNT: f32 = 0.35;
const REAR_CUTOFF: f32 = 4_000.0;
/// The level of each ear for a sound straight ahead: the vanilla panner's centred gain
/// (`stereo_gains(0, 0, 1)`), so turning Directional Audio on keeps sounds as loud.
const LEVEL: f32 = 0.5 + 0.095_660_8;

/// One ear's cues.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ear {
    /// The time difference, in frames.
    delay: f32,
    /// The head shadow's high-frequency gain.
    shadow: f32,
}

/// Where a sound is, as the two ears hear it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Params {
    /// Left, right.
    ears: [Ear; 2],
    /// The pinna reflection's delay, in frames.
    pinna: f32,
    /// How much of the highs the rear shadow takes, 0 to [`REAR_AMOUNT`].
    rear: f32,
    /// The sound's level: its gain and distance.
    gain: f32,
}

impl Params {
    /// A sound in direction `[right, up, forward]` (listener space, any length) at gain
    /// `gain`, for output rate `rate`.
    fn of(direction: [f32; 3], gain: f32, rate: f32) -> Self {
        let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
        let [x, y, z] = if length <= f32::EPSILON {
            [0.0, 0.0, 1.0]
        } else {
            direction.map(|v| (v / length).clamp(-1.0, 1.0))
        };
        // The angle off the median plane, and the far ear's extra path around the sphere.
        let lateral = x.abs().asin();
        let itd = HEAD_RADIUS / SPEED_OF_SOUND * (lateral + lateral.sin()) * rate;
        let ear = |axis: f32, far: bool| {
            let incidence = axis.clamp(-1.0, 1.0).acos().to_degrees();
            let shadow = (1.0 + SHADOW_DEEPEST / 2.0)
                + (1.0 - SHADOW_DEEPEST / 2.0)
                    * (incidence / SHADOW_ANGLE * std::f32::consts::PI).cos();
            Ear {
                delay: if far { itd } else { 0.0 },
                shadow,
            }
        };
        // The left ear faces -right, the right ear +right; a sound on the right reaches the
        // left ear last.
        let ears = [ear(-x, x > 0.0), ear(x, x < 0.0)];
        let below = (1.0 - y) * 0.5;
        Self {
            ears,
            pinna: (PINNA_ABOVE + (PINNA_BELOW - PINNA_ABOVE) * below) * rate,
            rear: REAR_AMOUNT * (-z).max(0.0),
            gain,
        }
    }

    fn lerp_step(from: &Self, to: &Self, n: f32) -> Self {
        let d = |a: f32, b: f32| (b - a) / n;
        Self {
            ears: std::array::from_fn(|i| Ear {
                delay: d(from.ears[i].delay, to.ears[i].delay),
                shadow: d(from.ears[i].shadow, to.ears[i].shadow),
            }),
            pinna: d(from.pinna, to.pinna),
            rear: d(from.rear, to.rear),
            gain: d(from.gain, to.gain),
        }
    }

    fn advance(&mut self, step: &Self) {
        for (ear, s) in self.ears.iter_mut().zip(step.ears) {
            ear.delay += s.delay;
            ear.shadow += s.shadow;
        }
        self.pinna += step.pinna;
        self.rear += step.rear;
        self.gain += step.gain;
    }
}

/// One voice's binaural state: the sound's recent samples, where its cues stand, and each
/// ear's filters.
#[derive(Clone, Debug)]
pub(super) struct Binaural {
    history: [f32; HISTORY],
    /// Where the next sample goes.
    at: usize,
    /// The cues the last buffer ended on; none while the voice isn't rendered binaurally.
    current: Option<Params>,
    /// Each ear's head-shadow filter state (last input, last output) and rear low-pass state.
    shadow: [(f32, f32); 2],
    rear: [f32; 2],
}

impl Default for Binaural {
    fn default() -> Self {
        Self {
            history: [0.0; HISTORY],
            at: 0,
            current: None,
            shadow: [(0.0, 0.0); 2],
            rear: [0.0; 2],
        }
    }
}

/// One buffer's glide, from [`Binaural::begin`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Glide {
    at: Params,
    step: Params,
    /// The head-shadow filter's constants at this rate: `T * K` (`T = a / 2c`, `K = 2 * rate`,
    /// the bilinear transform's) and the pole.
    tk: f32,
    pole: f32,
    /// The rear low-pass's coefficient.
    rear_a: f32,
}

impl Binaural {
    /// The voice was rendered another way this buffer: the next binaural buffer starts afresh
    /// rather than gliding from where this one left off.
    pub(super) fn release(&mut self) {
        if self.current.is_some() {
            *self = Self::default();
        }
    }

    /// Starts a buffer of `frames` frames for a sound in `direction` (listener space:
    /// right, up, forward) at `gain`.
    pub(super) fn begin(
        &mut self,
        direction: [f32; 3],
        gain: f32,
        frames: usize,
        rate: u32,
    ) -> Glide {
        let r = rate as f32;
        let target = Params::of(direction, gain, r);
        // A first buffer takes its place at once: there is nothing to glide from.
        let from = self.current.unwrap_or(target);
        self.current = Some(target);
        let tk = HEAD_RADIUS / (2.0 * SPEED_OF_SOUND) * 2.0 * r;
        Glide {
            at: from,
            step: Params::lerp_step(&from, &target, frames.max(1) as f32),
            tk,
            pole: (1.0 - tk) / (1.0 + tk),
            rear_a: 1.0 - (-std::f32::consts::TAU * REAR_CUTOFF / r).exp(),
        }
    }

    /// The sample `delay` frames (fractional) behind the newest, by Catmull-Rom interpolation.
    #[inline]
    fn read(&self, delay: f32) -> f32 {
        let p = (delay + LOOKAHEAD).clamp(LOOKAHEAD, (HISTORY - 3) as f32);
        let i = p as usize;
        let t = p - i as f32;
        let h = |back: usize| self.history[self.at.wrapping_sub(1 + back) & (HISTORY - 1)];
        let (y0, y1, y2, y3) = (h(i - 1), h(i), h(i + 1), h(i + 2));
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }

    /// One mono sample `x` in; `[left, right]` out.
    #[inline]
    pub(super) fn frame(&mut self, glide: &mut Glide, x: f32) -> [f32; 2] {
        self.history[self.at] = x;
        self.at = (self.at + 1) & (HISTORY - 1);
        let p = glide.at;
        let norm = (1.0 + glide.tk).recip();
        let mut out = [0.0; 2];
        for (e, ear) in p.ears.iter().enumerate() {
            let direct = self.read(ear.delay);
            let echo = self.read(ear.delay + p.pinna);
            let s = (direct + PINNA_GAIN * echo) / (1.0 + PINNA_GAIN);
            // H(s) = (1 + a s / 2w) / (1 + s / 2w), w = c / r: unity at low frequencies,
            // `shadow` at high ones.
            let b0 = (1.0 + ear.shadow * glide.tk) * norm;
            let b1 = (1.0 - ear.shadow * glide.tk) * norm;
            let (x1, y1) = self.shadow[e];
            let y = b0 * s + b1 * x1 - glide.pole * y1;
            self.shadow[e] = (s, y);
            self.rear[e] += (y - self.rear[e]) * glide.rear_a;
            out[e] = (y + (self.rear[e] - y) * p.rear) * p.gain * LEVEL;
        }
        glide.at.advance(&glide.step);
        out
    }
}

#[cfg(test)]
#[path = "binaural_tests.rs"]
mod tests;
