//! Blocking diagnostic reader for Scarlet native sensor devices.
//!
//! `sensor-dump` is intentionally a raw-data tool: it reports the ABI metadata
//! and sensor-native sample values without applying orientation or unit policy.

use std::process::ExitCode;

#[cfg(target_os = "scarlet")]
use std::env;

#[cfg(target_os = "scarlet")]
use scarlet_os::sensor::{SensorDevice, SensorEvent, SensorInfo};

#[cfg(not(target_os = "scarlet"))]
fn main() -> ExitCode {
    eprintln!("sensor-dump: this utility requires Scarlet OS");
    ExitCode::from(1)
}

#[cfg(target_os = "scarlet")]
fn usage() {
    eprintln!("usage: sensor-dump [/dev/sensorN] [count]");
}

#[cfg(target_os = "scarlet")]
fn parse_args() -> Result<(String, Option<usize>), &'static str> {
    let mut args = env::args().skip(1);
    let path = args.next().unwrap_or_else(|| String::from("/dev/sensor0"));
    let count = match args.next() {
        Some(value) => Some(
            value
                .parse::<usize>()
                .map_err(|_| "count must be a non-negative integer")?,
        ),
        None => None,
    };
    if args.next().is_some() {
        return Err("too many arguments");
    }
    Ok((path, count))
}

#[cfg(target_os = "scarlet")]
fn print_info(path: &str, info: SensorInfo) {
    println!("sensor={path}");
    println!(
        "info: type={:?} location={:?} chip={} axes={} resolution_bits={}",
        info.sensor_type, info.location, info.chip_id, info.axis_count, info.resolution_bits
    );
    println!(
        "raw: min={} max={} full_scale={}",
        info.raw_min, info.raw_max, info.full_scale
    );
    println!(
        "rate_millihz: min={} current={} max={}",
        info.min_frequency_millihz, info.current_frequency_millihz, info.max_frequency_millihz
    );
    println!("fifo_capacity={}", info.fifo_capacity);
}

#[cfg(target_os = "scarlet")]
fn print_event(event: SensorEvent) {
    println!(
        "event: timestamp_ns={} sequence={} xyz=[{}, {}, {}] flags=0x{:08x} lost={}",
        event.timestamp_ns,
        event.sequence,
        event.values[0],
        event.values[1],
        event.values[2],
        event.flags,
        event.lost_samples
    );
}

#[cfg(target_os = "scarlet")]
fn main() -> ExitCode {
    let (path, count) = match parse_args() {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("sensor-dump: {message}");
            usage();
            return ExitCode::from(2);
        }
    };

    let sensor = match SensorDevice::open(&path) {
        Ok(sensor) => sensor,
        Err(error) => {
            eprintln!("sensor-dump: failed to open {path}: {error:?}");
            return ExitCode::from(1);
        }
    };
    let info = match sensor.info() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("sensor-dump: failed to query {path}: {error:?}");
            return ExitCode::from(1);
        }
    };
    print_info(&path, info);

    let mut remaining = count;
    while remaining != Some(0) {
        // `SensorDevice` is opened in blocking mode, so this waits for a
        // complete record and never spins while the hardware is idle.
        match sensor.read_event() {
            Ok(Some(event)) => {
                print_event(event);
                if let Some(value) = remaining.as_mut() {
                    *value -= 1;
                }
            }
            Ok(None) => {
                eprintln!("sensor-dump: {path} closed its event stream");
                return ExitCode::from(1);
            }
            Err(error) => {
                eprintln!("sensor-dump: failed to read {path}: {error:?}");
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::SUCCESS
}
