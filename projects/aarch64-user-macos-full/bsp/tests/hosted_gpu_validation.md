# Hosted SGFX validation on Apple Silicon

On 2026-11-21 a Scarlet guest used the Khronos Vulkan loader, the Scarlet Vulkan ICD and the hosted SGFX guest driver to render a 64 × 64 SPIR-V triangle. The isolated fixture used 16 KiB-aligned ELF load ranges. The client waited for its fence, copied the image to a buffer and checked 112 interior, corner and orientation pixels.

| Host-side test condition | Metal texture readback | Guest check |
| --- | --- | --- |
| Submit the three-vertex draw | 1,152 red pixels and 2,944 blue pixels | All 112 checks passed |
| Omit only that draw and submit the remaining commands | 4,096 blue pixels | Failed at (30, 30), which remained blue |

Both runs used the same instrumented BSP binary and client. The test switch removed SGFX opcode 13 before submitting the command buffer to wgpu. The clear, GPU readback and guest checks ran in both cases. Production builds contain no draw-suppression switch.

The original teardown emitted SGFX status 2 during `vkDestroyPipeline`, after fence completion and readback. The dynamic SGFX wrapper returned `UNSUPPORTED` for the absent program-release extension. With the hosted driver's generational shader and pipeline release functions, both runs completed teardown cleanly.

In a separate run using ordinary ELF layouts, the Vulkan triangle client stopped in the packaged `libvulkan.so.1` at offset `0xb78a8`, in `__aarch64_swp4_rel`. The unmodified packaged `vulkan-canvas-demo --probe` stopped at executable offset `0x2fe01c` in `__aarch64_ldadd8_relax`. Both reported unsupported hosted boundary accesses. The packaged demo SHA-256 was `ffc687f98a0d87c56ebfc27828b95db53870f5812fd7c13f9ea2f81a55230cc1`. At this checkpoint the ordinary packaged guest and SWS dynamic compositor still needed independent runtime validation.

The BSP's `hosted_gpu` integration test exercises the Scarlet device controls, Metal execution, asynchronous completion, texture readback and an unaligned 11-byte logical-buffer read. Run it on an AArch64 Mac with `cargo test --locked --manifest-path projects/aarch64-user-macos-full/bsp/Cargo.toml --test hosted_gpu -- --ignored` from the repository root.

### Ordinary packaged guest checkpoint

In three independent headless boots on 2026-11-21, the packaged binaries, interpreter and hosted SGFX DSO were copied from the rebuilt project's staging tree into private disks. The DSO exported BackendApi, DriverApi and ProgramReleaseApi. Each executable stopped on the same `ldxr x0, [x1]` instruction (`0xc85f7c20`) in the relaxed 64-bit fetch-add helper, before opening the GPU.

| Staged executable | Guest PC | Operand address |
| --- | --- | --- |
| `sws` | `0x70001d0e38` | `0x70001f9400` |
| `sgfx-probe` | `0x7000030fd0` | `0x7000051f20` |
| `vulkan-canvas-demo --probe` | `0x70002fe01c` | `0x700033a608` |

The SWS boot isolates startup. A subsequent compositor test needs a display and a client surface. SWS creates imported textures with `SAMPLED | COPY_SRC`; the pinned wgpu `map_image` requires `RENDER_ATTACHMENT | PRESENT`.

After rebuilding the BSP with scalar exclusive-window mediation, the same unmodified packaged `sgfx-probe` selected `scarlet-hosted`, identified `/lib/sgfx/libsgfx_hosted.so` as dynamically loaded and created a graphics context. Packaged SWS reached compositor initialization and stopped at display discovery in the headless boot. The Vulkan probe enumerated the hosted physical device and created a logical Vulkan device.

### Canonical full-image acceptance, 2026-11-21

The graphical 19:32:30 release BSP booted the project's full cpio (SHA-256 `9b4847f382bd1f206de2af67fe772b8fda619df5fb952406211a08383f574f89`) and a private APFS clone of the 19:24:58 rootfs (source inode 385744305). After an ordinary console login, the packaged `/bin/sgfx-probe` selected the dynamic `/lib/sgfx/libsgfx_hosted.so` and reported rendering, presentation and a graphics context. In the same login, the packaged `/bin/vulkan-canvas-demo --probe` enumerated `SGFX Vulkan (Scarlet scarlet-hosted GPU 0)`. This graphical run timed out before a cube readback.

A separate diagnostic boot of the packaged Vulkan client and DSO identified the failure in image export. At `vkGetImageScarletHandleSGFX`, the ICD passed a 512 × 512 BGRA8 logical texture with `RENDER_ATTACHMENT | COPY_SRC` to the hosted driver. The pinned backend's presentation mapping returned `InvalidState`, which surfaced as `Dynamic(Status(1))` and `ERROR_FEATURE_NOT_PRESENT`. This happened before the first cube draw. It is distinct from the earlier triangle teardown status 2.

The SGFX shared-image regression binds a producer's non-presentable color target and a consumer's sampled texture to one Metal image, writes two distinct texels, drops the producer, and verifies both texels through the consumer. It also rejects a writable sampled import. The upstream regression and the BSP's Metal guest-controls integration test passed with the shared-image change.

On the release BSP using the SGFX Git revision `c373416e8dc3d86e7ff1bc397d6b5383aa093409`, the packaged cube passed a 512 × 512 textured GPU readback with 72,005 foreground pixels and 1,240 colors. This diagnostic used a private clone of the full rootfs, a direct test launcher in place of stemd, and a hosted DSO built with bulk upload packing. The client SHA-256 matched the packaged executable above. The separate full-system run using the original packaged DSO reached command recording and was stopped when GUI measurement took priority.


### Hosted driver upload packing

Two samples from the original hosted DSO landed at guest PCs `0x7b00647d04` and `0x7b00647d68` inside its `memcpy`. The first sample included return address `0x7b0062a284`, immediately after a call from `sgfx_hosted::recording` at DSO offset `0x1a280`. The original packing loop called the copy routine for each eight-byte chunk. For a 1 MiB payload that loop performs 131,072 copies. The updated driver performs one bulk copy into the padded wire payload. Upload collection still copies guest data into an owned staging vector.

The isolated bulk-copy DSO completed both draws and teardown in the first Vulkan probe cycle. Readback verified 72,005 and 72,475 foreground pixels, with 1,240 colors at each angle. The test was stopped during the next cycle when normal GUI measurement took priority. These runs did not measure matched frame latency or establish whether the stock greeter waited in upload packing, queue admission, or presentation.
