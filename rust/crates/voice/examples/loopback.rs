//! Checks stream audio capture: prints how loud everything playing on this PC is, for a few seconds. Play
//! something while it runs.

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};
    use swiftcord_voice::loopback::{to_float, Capture, Target};

    let mut capture = Capture::start(Target::AllButUs).expect("start capture");
    let start = Instant::now();
    let (mut bytes, mut samples, mut peak, mut total) = (Vec::new(), Vec::new(), 0f32, 0usize);
    while start.elapsed() < Duration::from_secs(5) {
        bytes.clear();
        samples.clear();
        assert!(capture.read(Duration::from_millis(100), &mut bytes), "capture failed");
        to_float(&bytes, &mut samples);
        total += samples.len();
        peak = samples.iter().fold(peak, |p, s| p.max(s.abs()));
    }
    println!("captured {:.2} s of stereo, peak {peak:.3}", total as f32 / 2.0 / 48_000.0);
}

#[cfg(not(windows))]
fn main() {}
