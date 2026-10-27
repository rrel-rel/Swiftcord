//! Lists audio devices and runs the mic test for a few seconds, printing the input level and whether the
//! voice gate is open: `cargo run -p swiftcord-voice --example devices`.

use std::time::{Duration, Instant};

fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    let (inputs, outputs) = swiftcord_voice::list_devices();
    println!("inputs:  {inputs:?}");
    println!("outputs: {outputs:?}");

    let test = swiftcord_voice::MicTest::start(Default::default(), -50.0, true);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(500));
        println!("level {:6.1} dBFS, transmitting: {}", test.level_db(), test.transmitting());
    }
    for e in test.errors() {
        println!("error: {e}");
    }
}
