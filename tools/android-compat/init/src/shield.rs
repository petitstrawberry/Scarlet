//! Explicit userspace policy for the stock libselinux validation fixture.
use std::{
    environment::VfsView,
    shield::{Policy, Rule, Transition},
};
pub fn configure(android: &VfsView) -> Result<(), &'static str> {
    let policy = if option_env!("SCARLET_ANDROID_ZYGOTE_POLICY") == Some("1") {
        Policy::create_with_transitions(&[
            (1, "u:r:zygote:s0"),
            (2, "u:r:untrusted_app:s0:c1,c256,c512,c768"),
            (3, "u:r:untrusted_app:s0:c2,c256,c512,c768"),
        ], &[], &[
            Transition {source:1,target:2,principal:10001,reserved:0},
            Transition {source:1,target:3,principal:10002,reserved:0},
        ])
    } else { Policy::create(
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
    ) }
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
