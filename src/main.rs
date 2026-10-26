use fiio_monitor::{devices, Monitor, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() -> Result<()> {
    let mut input_name = "KT-USB-Audio".to_owned();
    let mut output_name = "MacBook Pro Speakers".to_owned();
    let mut gain_db = -6.0_f32;
    let mut latency_ms = 150_usize;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--list" => {
                let devices = devices()?;
                for name in devices.inputs {
                    println!("Input:  {name}");
                }
                for name in devices.outputs {
                    println!("Output: {name}");
                }
                return Ok(());
            }
            "--input" => input_name = arguments.next().ok_or("Missing input name")?,
            "--output" => output_name = arguments.next().ok_or("Missing output name")?,
            "--gain-db" => gain_db = arguments.next().ok_or("Missing gain")?.parse()?,
            "--latency-ms" => latency_ms = arguments.next().ok_or("Missing latency")?.parse()?,
            "--help" | "-h" => {
                println!("fiio-monitor [--list] [--input NAME] [--output NAME] [--gain-db DB] [--latency-ms MS]\nDefaults: KT-USB-Audio -> MacBook Pro Speakers, -6 dB, 150 ms.\nPress Enter to stop. No audio files are created.");
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {argument}").into()),
        }
    }
    let monitor = Monitor::start(&input_name, &output_name, gain_db, latency_ms)?;
    println!(
        "{input_name} -> {output_name}; stereo 48000 Hz; {gain_db} dB; buffer {latency_ms} ms"
    );
    println!("No recording. Press Enter to stop. Start with low speaker volume.");
    let stopped = Arc::new(AtomicBool::new(false));
    let keyboard_stop = stopped.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        keyboard_stop.store(true, Ordering::Relaxed);
    });
    while !stopped.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        let readings = monitor.readings();
        let peak = readings.peak;
        eprint!("\rInput peak: {:6.1} dBFS | near-full-scale frames: {} | buffer lost: {} | gaps: {} | stream errors: {}    ",
            20.0 * peak.log10(), readings.clipped,
            readings.overflow, readings.underrun,
            readings.errors);
        if readings.errors > 0 {
            eprintln!("\nAudio device error; check connection and restart.");
            break;
        }
    }
    drop(monitor);
    println!("\nStopped.");
    Ok(())
}
