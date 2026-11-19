use super::*;

const RATE: u32 = 48_000;

/// White noise, the same every run.
fn noise(frames: usize) -> Vec<f32> {
    let mut seed = 7u32;
    (0..frames)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        })
        .collect()
}

/// `signal` heard from `direction`, in 480-frame buffers: `[left, right]` per frame.
fn render(direction: [f32; 3], signal: &[f32]) -> Vec<[f32; 2]> {
    let mut voice = Binaural::default();
    let mut out = Vec::with_capacity(signal.len());
    for chunk in signal.chunks(480) {
        let mut glide = voice.begin(direction, 1.0, chunk.len(), RATE);
        out.extend(chunk.iter().map(|&x| voice.frame(&mut glide, x)));
    }
    out
}

/// Energy above about 6 kHz of one ear, in dB: a first difference is a gentle high-pass.
fn highs_db(out: &[[f32; 2]], ear: usize) -> f32 {
    let e: f32 = out
        .windows(2)
        .map(|w| (w[1][ear] - w[0][ear]).powi(2))
        .sum();
    10.0 * e.log10()
}

#[test]
fn mirrored_sounds_swap_the_ears_exactly() {
    let signal = noise(9600);
    for direction in [
        [0.7, 0.2, 0.4],
        [1.0, 0.0, 0.0],
        [0.3, -0.8, -0.5],
        [0.05, 0.9, 0.1],
    ] {
        let [x, y, z] = direction;
        let a = render([x, y, z], &signal);
        let b = render([-x, y, z], &signal);
        for (a, b) in a.iter().zip(&b) {
            assert_eq!(a[0].to_bits(), b[1].to_bits());
            assert_eq!(a[1].to_bits(), b[0].to_bits());
        }
    }
}

#[test]
fn a_sound_ahead_reaches_both_ears_alike_at_the_vanilla_level() {
    let out = render([0.0, 0.0, 1.0], &noise(4800));
    assert!(out.iter().all(|f| f[0].to_bits() == f[1].to_bits()));
    // A low tone ahead plays at the stereo panner's centred gain.
    let tone: Vec<f32> = (0..9600)
        .map(|i| (std::f32::consts::TAU * 100.0 * i as f32 / RATE as f32).sin())
        .collect();
    let out = render([0.0, 0.0, 1.0], &tone);
    let peak = out[4800..].iter().fold(0.0f32, |p, f| p.max(f[0].abs()));
    assert!((peak - LEVEL).abs() < 0.02 * LEVEL, "{peak}");
}

#[test]
fn the_far_ear_hears_a_side_sound_later_by_the_path_around_the_head() {
    let signal = noise(9600);
    let out = render([1.0, 0.0, 0.0], &signal);
    // The lag of the best match between the ears is Woodworth's (a / c)(pi / 2 + 1).
    let expected = HEAD_RADIUS / SPEED_OF_SOUND * (std::f32::consts::FRAC_PI_2 + 1.0) * RATE as f32;
    let lag = (0..64)
        .max_by(|&a, &b| {
            let c = |lag: usize| -> f32 { (2000..9000).map(|i| out[i][1] * out[i + lag][0]).sum() };
            c(a).total_cmp(&c(b))
        })
        .unwrap();
    assert!((lag as f32 - expected).abs() <= 2.0, "{lag} vs {expected}");
}

#[test]
fn the_head_shadows_the_far_ears_highs() {
    let signal = noise(9600);
    let side = render([1.0, 0.0, 0.0], &signal);
    assert!(highs_db(&side, 1) - highs_db(&side, 0) > 12.0);
    // Behind, both ears lose some highs against a sound ahead.
    let ahead = render([0.0, 0.0, 1.0], &signal);
    let behind = render([0.0, 0.0, -1.0], &signal);
    assert!(highs_db(&ahead, 0) - highs_db(&behind, 0) > 1.0);
}

#[test]
fn turning_the_head_glides_without_a_click() {
    // A 500 Hz tone while the listener spins once a second, a new direction every 10 ms buffer:
    // no frame moves further than the tone itself can, lifted by the near ear's shadow.
    let mut voice = Binaural::default();
    let mut last = [0.0f32; 2];
    let mut jump = 0.0f32;
    for buffer in 0..200 {
        let angle = buffer as f32 * std::f32::consts::TAU / 100.0;
        let mut glide = voice.begin([angle.sin(), 0.0, angle.cos()], 1.0, 480, RATE);
        for f in 0..480 {
            let i = buffer * 480 + f;
            let x = (std::f32::consts::TAU * 500.0 * i as f32 / RATE as f32).sin();
            let out = voice.frame(&mut glide, x);
            if buffer > 0 {
                jump = jump
                    .max((out[0] - last[0]).abs())
                    .max((out[1] - last[1]).abs());
            }
            last = out;
        }
    }
    let slope = std::f32::consts::TAU * 500.0 / RATE as f32;
    assert!(jump < slope * LEVEL * 2.5, "{jump}");
}

#[test]
fn any_direction_stays_bounded() {
    let mut voice = Binaural::default();
    let signal = noise(48_000);
    let mut seed = 3u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let mut loudest = 0.0f32;
    for chunk in signal.chunks(64) {
        let direction = [next(), next(), next()];
        let mut glide = voice.begin(direction, 1.0, chunk.len(), RATE);
        for &x in chunk {
            let out = voice.frame(&mut glide, x);
            loudest = loudest.max(out[0].abs()).max(out[1].abs());
        }
    }
    assert!(loudest.is_finite() && loudest < 3.0, "{loudest}");
    // A sound at the listener's own position is heard as ahead.
    assert_eq!(
        Params::of([0.0; 3], 1.0, 48_000.0),
        Params::of([0.0, 0.0, 2.0], 1.0, 48_000.0)
    );
}

#[test]
fn the_delays_fit_the_history_at_every_rate() {
    for rate in [8_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let p = Params::of([1.0, -1.0, 0.0], 1.0, rate);
        let longest = p.ears[0].delay + p.pinna + LOOKAHEAD + 2.0;
        assert!(longest < HISTORY as f32, "{rate}: {longest}");
    }
}
