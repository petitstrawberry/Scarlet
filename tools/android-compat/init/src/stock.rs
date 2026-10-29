//! Fixture-only stock property snapshot and linker configuration bootstrap.
use std::{
    environment::{Environment, HandleMapping, VfsView},
    println,
};

pub fn environment(
    native: VfsView,
) -> Result<(Environment, Option<std::shield::Policy>), &'static str> {
    let android = native
        .rooted_at("/systems/android-aarch64")
        .map_err(|_| "stock Android view")?;
    // DevFS itself cannot create files. Give this fixture's Android view its
    // own writable device overlay; the global/native device tree stays shared
    // only as a lower layer. No Android property service is implied.
    let upper = VfsView::create("tmpfs", "size=64M").map_err(|_| "property storage")?;
    let devices = VfsView::overlay(&native, "/dev", Some((&upper, "/")))
        .map_err(|_| "Android device overlay")?;
    devices
        .create_directory("/__properties__")
        .map_err(|_| "property directory")?;
    devices
        .create_directory("/socket")
        .map_err(|_| "log socket directory")?;
    android
        .bind("/dev", &devices, "/")
        .map_err(|_| "Android devices")?;
    // A separate bind gives property files real TmpFS inode identity rather
    // than overlay nodes; unsupported filesystems keep rejecting xattrs.
    android
        .bind("/dev/__properties__", &upper, "/__properties__")
        .map_err(|_| "property storage bind")?;
    android
        .bind("/dev/pts", &native, "/dev/pts")
        .map_err(|_| "Android ptys")?;
    android
        .bind("/tmp", &native, "/tmp")
        .map_err(|_| "Android tmp")?;
    if option_env!("SCARLET_ANDROID_SHIELD") == Some("1") {
        super::shield::configure(&android)?;
    }
    let policy = if option_env!("SCARLET_ANDROID_SERVICES") == Some("1") {
        Some(super::services::configure(&android)?)
    } else {
        None
    };
    let env = Environment::create().map_err(|_| "stock Environment")?;
    env.set_root("scarlet", &native)
        .map_err(|_| "native view")?;
    env.set_root("android-aarch64", &android)
        .map_err(|_| "Android view")?;
    env.seal().map_err(|_| "seal stock Environment")?;
    Ok((env, policy))
}

fn run(
    env: &Environment,
    view: &VfsView,
    handles: &[HandleMapping<'_>],
    name: &str,
    argv: &[&str],
) -> Result<(), &'static str> {
    let executable = view.open(argv[0], 0).map_err(|_| "bootstrap executable")?;
    println!("\nSCARLET_ANDROID_STOCK_START {}", name);
    let pid = env
        .spawn_with_abi(
            "android-aarch64",
            &executable,
            argv,
            &["ANDROID_LOG_TAGS=*:v"],
            "/",
            handles,
        )
        .map_err(|_| "bootstrap spawn")?;
    let (waited, status) = std::task::waitpid(pid as i32, 0);
    println!("SCARLET_ANDROID_STOCK_EXIT {} {}", name, status);
    if waited != pid as i32 || status != 0 {
        return Err("stock bootstrap wait or exit");
    }
    Ok(())
}

pub fn prepare(
    env: &Environment,
    view: &VfsView,
    handles: &[HandleMapping<'_>],
) -> Result<(), &'static str> {
    run(
        env,
        view,
        handles,
        "properties",
        &[
            "/data/local/tmp/property-bootstrap",
            "/data/local/tmp/property-contexts.list",
            "/data/local/tmp/property-values.prop",
        ],
    )?;
    run(
        env,
        view,
        handles,
        "linkerconfig",
        &[
            "/apex/com.android.runtime/bin/linkerconfig",
            "--target",
            "/linkerconfig",
        ],
    )?;
    let file = view
        .open("/linkerconfig/com.android.art/ld.config.txt", 0)
        .map_err(|_| "ART linker configuration missing")?;
    let mut data = std::vec![0; 128 * 1024];
    let stream = file
        .as_stream()
        .map_err(|_| "linker configuration stream")?;
    let mut count = 0;
    loop {
        let n = stream
            .read(&mut data[count..])
            .map_err(|_| "linker configuration read")?;
        if n == 0 {
            break;
        }
        count += n;
        if count == data.len() {
            return Err("linker configuration too large");
        }
    }
    // Local datagram delivery to Android logd is not implemented yet. Match
    // the entire stock-generated ART configuration captured by the passing
    // host diagnostic instead of trusting the generator's zero exit status.
    // This is verification data only: the target still generates its own file.
    const EXPECTED: &str =
        include_str!("../../checkpoints/2026-10-28-cloud/stock-art/generated-art-ld.config.txt");
    if data[..count] != *EXPECTED.as_bytes() {
        return Err("stock ART linker configuration differs from reference");
    }
    println!("SCARLET_ANDROID_STOCK_OK");
    Ok(())
}
