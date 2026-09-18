//! Inspect a GPU through the ordinary Scarlet control ABI.

#[cfg(target_os = "scarlet")]
fn run() -> Result<(), String> {
    use gpu_raw::{
        GPU_DEVICE_STATE_LOST, GPU_DEVICE_STATE_READY, GPU_EXECUTION_SUPPORT_ADDRESS_SPACE,
        GPU_EXECUTION_SUPPORT_DEPTH, GPU_EXECUTION_SUPPORT_IMAGE_READBACK,
        GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD, GPU_EXECUTION_SUPPORT_MEMORY,
        GPU_EXECUTION_SUPPORT_PRESENTATION, GPU_EXECUTION_SUPPORT_QUEUE,
        GPU_EXECUTION_SUPPORT_TIMELINE, GPU_RESULT_SUCCESS, Gpu,
    };

    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [] => "/dev/gpu0",
        [path] => path.as_str(),
        _ => return Err("usage: gpu-info [/dev/gpuN]".into()),
    };
    let gpu = Gpu::open(path).map_err(|error| format!("{path}: {error:?}"))?;
    let info = gpu
        .query_info()
        .map_err(|error| format!("query: {error:?}"))?;
    if info.result != GPU_RESULT_SUCCESS {
        return Err(format!("query result: {}", info.result));
    }
    println!("device: {path}");
    println!(
        "backend: {}",
        String::from_utf8_lossy(info.backend_id_bytes())
    );
    let state = match info.device_state {
        GPU_DEVICE_STATE_READY => "ready",
        GPU_DEVICE_STATE_LOST => "lost",
        gpu_raw::GPU_DEVICE_STATE_UNAVAILABLE => "unavailable",
        _ => "unknown",
    };
    println!("state: {state}");
    println!("execution support: {:#x}", info.execution_support);
    for (name, flag) in [
        ("address space", GPU_EXECUTION_SUPPORT_ADDRESS_SPACE),
        ("memory", GPU_EXECUTION_SUPPORT_MEMORY),
        ("queue", GPU_EXECUTION_SUPPORT_QUEUE),
        ("timeline", GPU_EXECUTION_SUPPORT_TIMELINE),
        ("presentation", GPU_EXECUTION_SUPPORT_PRESENTATION),
        ("image upload", GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD),
        ("image readback", GPU_EXECUTION_SUPPORT_IMAGE_READBACK),
        ("depth", GPU_EXECUTION_SUPPORT_DEPTH),
    ] {
        println!("  {name}: {}", info.execution_support & flag != 0);
    }
    println!("max command bytes: {}", info.max_opaque_command_size);
    println!("backend feature bits: {:#x}", info.backend_feature_bits);
    print!("backend info:");
    for byte in info.backend_info_bytes() {
        print!(" {byte:02x}");
    }
    println!();
    if info.execution_support & GPU_EXECUTION_SUPPORT_QUEUE != 0 {
        let dialect = gpu
            .query_dialect(0)
            .map_err(|error| format!("dialect 0: {error:?}"))?;
        println!(
            "dialect 0: {}",
            String::from_utf8_lossy(dialect.opaque_info())
        );
    }
    Ok(())
}

#[cfg(not(target_os = "scarlet"))]
fn run() -> Result<(), String> {
    Err("the Scarlet GPU control ABI is required".into())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gpu-info: {error}");
            std::process::ExitCode::from(1)
        }
    }
}
