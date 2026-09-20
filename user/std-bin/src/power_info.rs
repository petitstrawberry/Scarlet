//! Inspect the same read-only telemetry used by the desktop battery icon.
#[cfg(target_os = "scarlet")]
fn run() -> Result<(), String> {
    use scarlet_os::power_supply::PowerSupplies;
    let supplies = PowerSupplies::open().map_err(|e| format!("open: {e:?}"))?;
    let count = supplies.count().map_err(|e| format!("enumerate: {e:?}"))?;
    println!("power supplies: {count}");
    for id in 0..count {
        let sample = supplies
            .snapshot(id)
            .map_err(|e| format!("supply {id}: {e:?}"))?;
        println!("{id}: {} ({:?})", sample.name(), sample.kind);
        if sample.read_failed {
            println!("  unavailable: hardware read failed");
            continue;
        }
        let state = sample.state;
        println!(
            "  present={:?} online={:?} charge={:?}",
            state.present, state.online, state.charge_state
        );
        if let Some(value) = state.capacity_permille {
            println!("  capacity={}.{}%", value / 10, value % 10);
        }
        if let Some(value) = state.voltage_uv {
            println!("  voltage_uv={value}");
        }
        if let Some(value) = state.current_ua {
            println!("  battery_current_ua={value} (positive into battery)");
        }
        if let Some(value) = state.temperature_mc {
            println!("  temperature_mc={value}");
        }
        if let Some(value) = state.input_current_limit_ua {
            println!("  input_current_limit_ua={value} (configured limit)");
        }
    }
    Ok(())
}
#[cfg(not(target_os = "scarlet"))]
fn run() -> Result<(), String> {
    Err("Scarlet power-supply API required".into())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("power-info: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
