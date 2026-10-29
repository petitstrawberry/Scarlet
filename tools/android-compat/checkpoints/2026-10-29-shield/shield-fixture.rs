//! Explicit userspace policy for the stock libselinux validation fixture.
use std::{
    environment::VfsView,
    shield::{Policy, Rule},
};
pub fn configure(android: &VfsView) -> Result<(), &'static str> {
    let policy = Policy::create(
        &[
            (1, "u:r:scarlet_probe:s0"),
            (2, "u:r:scarlet_denied:s0"),
            (3, "u:object_r:scarlet_service:s0"),
        ],
        &[
            Rule {
                source: 1,
                target: 3,
                class: 1,
                reserved: 0,
                permissions: 3,
            },
            Rule {
                source: 1,
                target: 1,
                class: 1,
                reserved: 0,
                permissions: 4,
            },
        ],
    )
    .map_err(|_| "Shield policy creation")?;
    let fs = VfsView::create(
        "android-selinux",
        &std::format!("policy={}", policy.as_handle().as_raw()),
    )
    .map_err(|_| "Shield Android security view")?;
    android
        .bind("/sys/fs/selinux", &fs, "/")
        .map_err(|_| "Shield mount")?;
    policy.apply(1).map_err(|_| "Shield subject assignment")?;
    if Policy::current()
        .map_err(|_| "Shield policy query")?
        .apply(2)
        .is_ok()
    {
        return Err("Shield query granted assignment authority");
    }
    std::println!("\nSCARLET_SHIELD_CONFIGURED");
    Ok(())
}
