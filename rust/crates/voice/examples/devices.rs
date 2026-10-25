//! Lists audio devices and plays a one-second tone through the mixer while capturing the microphone, to
//! check audio I/O on a machine without joining a call: `cargo run -p swiftcord-voice --example devices`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use swiftcord_voice::audio::{self, Mixer};

fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
    let (inputs, outputs) = swiftcord_voice::list_devices();
    println!("inputs:  {inputs:?}");
    println!("outputs: {outputs:?}");

    let (mic_tx, mic_rx) = std::sync::mpsc::sync_channel(64);
    let mixer = Arc::new(Mixer::default());
    let (io, errors) = audio::start(Default::default(), mic_tx, mixer.clone(), Arc::new(AtomicBool::new(false)));

    // A quiet 440 Hz tone, fed in 20 ms frames.
    let start = Instant::now();
    let mut captured = 0usize;
    let mut peak = 0f32;
    let mut phase = 0f32;
    while start.elapsed() < Duration::from_secs(1) {
        let frame: Vec<f32> = (0..960)
            .flat_map(|_| {
                phase += 440.0 * std::f32::consts::TAU / 48_000.0;
                let s = phase.sin() * 0.1;
                [s, s]
            })
            .collect();
        mixer.push(1, &frame);
        while let Ok(chunk) = mic_rx.try_recv() {
            captured += chunk.len();
            peak = chunk.iter().fold(peak, |p, s| p.max(s.abs()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(io);
    for e in errors.try_iter() {
        println!("error: {e}");
    }
    println!("captured {captured} mic samples at 48 kHz (~{:.2} s), peak level {peak:.3}", captured as f32 / 48_000.0);
}
