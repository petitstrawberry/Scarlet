# First Home diagnostics

For a diagnostic boot, set `SCARLET_HOME_TRACE=1` in **both** the `sws` and
`scarlet-shell` service environments. It is disabled by default. No UI or GPU
dependency revision changes are required. Capture service stdout to a file;
synchronous UART output can alter the scheduling being measured.

When using the normal supervised session, set the flag on `scarlet-desktop`:
it forwards only `SCARLET_HOME_TRACE=1` to its shell child and respawns. Other
environment variables and the desktop-settings child retain their normal policy.

Each process emits at most 24 records per boundary: occurrences 1–16 and powers
of two through 4096. Counts are independent for each boundary and reset on process
restart. All records use the OS monotonic nanosecond clock, so shell and SWS
records can be ordered together. SWS records identify only the window titled
`Home`; they do not enumerate or read back client GPU pixels.

| Stage | Boundary |
| --- | --- |
| 0 | Shell receives authoritative workspace presentation |
| 1 | Shell builds Drawer geometry and application count |
| 2 | Drawer element actually lays out with its reconciled presentation |
| 3 | SWS begins/ends shared image registration, including identity and extent |
| 4 | SWS begins/ends COMMIT acceptance, including identity, serial and damage |
| 5 | SWS chooses scene/shared/SHM source, with pending/presented tokens |
| 6 | GPU composition and display present return successfully |
| 7 | SWS sends FRAME_DONE with its present counter and callback target |
| 8 | Home projection geometry, clip, opacity, instance and retained transition |
| 9 | Shell runner synchronizes the Home platform window |

Boot fresh, open Home directly once, and collect 2/4/15-second output captures
without intervening app launches or Overview. Preserve a later capture and its
elapsed time separately; record any human interaction. Compare damage-independent
display readback to RFB before attributing an old image to producer rendering.

These records locate the shell/layout/COMMIT/composition boundary. They do not
directly instrument UI GPU execution or FRAME_DONE receipt: in UI revision
`8efde207`, successful GPU completion precedes REGISTER and COMMIT. If layout
advances but no COMMIT arrives, instrument that producer interval next. Avoid
enabling the unbounded `SCARLET_UI_PROFILE` or `SWS_PROFILE` logs for a long run.
