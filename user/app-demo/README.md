# App Demo

A native ScarletUI application packaged as `/applications/app-demo.app`.
It opens a window, reads `resources/message.txt` from the installed application
and has an interactive counter. `app.toml` owns the application ID, name,
executable, icon and Cargo recipe; `build.rs` derives the window identity from it.

The normal desktop bundle contains an `app` layer for this source. Inside the
Scarlet development shell, build the usual project image:

```sh
cargo scarlet image --project projects/aarch64-limine-full --image rootfs --release
```

The SDK builds the executable and packages the resources and generated runtime
`org.scarlet-os.app-demo.desktop` automatically. No prebuilt executable or second
editable desktop descriptor is needed. Rebuild the boot/disk images normally
when using the project runner; a rootfs-only build does not replace the boot disk.
Launch **App Demo** from the application catalog after the normal desktop boots.
The demo never modifies other installed applications or the catalog files.

For a finished application directory without an image:

```sh
cargo scarlet app build --source user/app-demo \
  --target aarch64-unknown-scarlet --release --output target/app-demo.app
```

Use a new output directory for each app build. The source supports the native
AArch64 and RISC-V64 targets; each output contains one target. The image build
uses the selected project's native SGFX configuration. Direct app builds need
the same native linker configuration in their Cargo environment.
