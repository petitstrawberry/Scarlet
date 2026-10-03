#!/usr/bin/env python3
"""Run production controls, overlay rendering, and stream codec gates on the host.

Scarlet syscalls and the decoder cannot run on the host. Extract the actual
control items, with only platform I/O stubbed, into a disposable Cargo package.
Use the same pinned ScarletUI revision as VideoPlayer, without sibling checkouts.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="Rust host target; defaults to rustc's host")
    args = parser.parse_args()
    player = Path(__file__).resolve().parents[1]
    repository = player.parents[1]
    target = Path(os.environ.get("CARGO_TARGET_DIR", repository / "target")).resolve()
    output = target / "video-player-controls-host"
    output.mkdir(parents=True, exist_ok=True)
    source = (player / "src/main.rs").read_text()

    def section(start, end):
        first = source.index(start)
        return source[first:source.index(end, first)]

    prefixes = ("DISPLAY_", "CONTROLS_", "PLAY_BUTTON_", "LOOP_BUTTON_", "SEEK_TRACK_", "SEEK_KNOB_")
    constants = "\n".join(
        line for line in source.splitlines()
        if any(line.startswith("const " + prefix) for prefix in prefixes)
    )
    shared = json.dumps(str(player / "src/shared_u64.rs"))
    code = """#![feature(portable_simd)]
#![allow(dead_code)]
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use std::thread;
use std::sync::Arc;
use std::simd::Simd;
use scarlet_ui::{Buffer, Canvas, Color, ColorPalette};
struct SharedVideoFrame;
use scarlet_ui::{graphics, Event, MouseEvent, MouseButton, KeyEvent, KeyCode, InteractionMode};
use scarlet_ui::event::{TouchChange, TouchPhase};
struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
static TOUCH_MODE: AtomicBool = AtomicBool::new(false);
fn current_input_environment() -> scarlet_ui::InputEnvironment {
    scarlet_ui::InputEnvironment::new(
        1, Some(TOUCH_MODE.load(Ordering::Relaxed)), None, true, true, true, false)
}
static DISMISSALS: AtomicU32 = AtomicU32::new(0);
fn dismiss_window(_: &str) { DISMISSALS.fetch_add(1, Ordering::Relaxed); }
struct PaintSignal;
impl PaintSignal { fn notify(&self) {} }
""" + f"#[path={shared}] mod shared_u64;\nuse shared_u64::SharedU64;\n"
    code += constants + "\n"
    for start, end in (
        ("struct VideoFrameData {", "impl VideoFrameStore {"),
        ("struct ControlsOverlay {", "struct PaintSignal {"),
        ("fn pointer_control(", "fn draw_debug_overlay("),
        ("fn draw_debug_overlay(", "fn fit_size("),
        ("fn fill_bgra(", '#[unsafe(no_mangle)]'),
        ("enum VideoCodec {", "impl VideoCodec {"),
        ("fn streaming_hardware_codec_supported(", "impl VideoSource {"),
    ):
        code += section(start, end)
    video_view = (player / "src/video_view.rs").read_text()
    code += video_view[video_view.index("fn buffer("):video_view.index("impl ElementRenderObject for VideoRender {")]
    code += (player / "tests/host/controls.rs").read_text()
    (output / "controls.rs").write_text(code)
    ui_line = next(line for line in (player / "Cargo.toml").read_text().splitlines()
                   if line.startswith("scarlet-ui = "))
    url = re.search(r'git\s*=\s*"([^"]+)"', ui_line).group(1)
    revision = re.search(r'rev\s*=\s*"([^"]+)"', ui_line).group(1)
    (output / "Cargo.toml").write_text("""[package]
name = "video-player-controls-tests"
version = "0.0.0"
edition = "2024"
[workspace]
[lib]
path = "controls.rs"
[features]
h264-stateful-hw = []
h264-stateless-hw = []
hevc-stateful-hw = []
[dependencies]
scarlet-ui = { git = """ + json.dumps(url) + ", rev = " + json.dumps(revision) +
        ', default-features = false, features = ["std"] }\n')
    host = args.target
    if not host:
        version = subprocess.check_output(["rustc", "-vV"], text=True)
        host = next(line.split(": ", 1)[1] for line in version.splitlines()
                    if line.startswith("host: "))
    for features in ["h264-stateful-hw,hevc-stateful-hw", "h264-stateless-hw", ""]:
        subprocess.run([
            "cargo", "test", "--manifest-path", str(output / "Cargo.toml"),
            "--target", host, "--features", features, "--", "--test-threads=1",
        ], check=True)


if __name__ == "__main__":
    main()
