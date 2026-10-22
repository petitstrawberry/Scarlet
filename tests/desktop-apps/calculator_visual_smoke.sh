#!/usr/bin/env bash
set -euo pipefail
export SCARLET_UI_WINIT_RENDERER=cpu
export WINIT_X11_SCALE_FACTOR=1
out=${SCARLET_VISUAL_OUTPUT:-$(mktemp -d /tmp/scarlet-calculator-visual.XXXXXX)}
mkdir -p "$out"
display_file=$(mktemp "$out/display.XXXXXX")
Xvfb -displayfd 3 -screen 0 1280x1100x24 -nolisten tcp 3> "$display_file" > "$out/xvfb.log" 2>&1 &
xvfb_pid=$!
app_pid=
trap 'if [ -n "$app_pid" ]; then kill "$app_pid" 2>/dev/null || true; fi; kill "$xvfb_pid" 2>/dev/null || true; rm -f "$display_file"' EXIT
for i in $(seq 1 40); do [ ! -s "$display_file" ] || break; sleep .1; done
export DISPLAY=":$(cat "$display_file")"
"${SCARLET_CALCULATOR_BINARY:?Set SCARLET_CALCULATOR_BINARY to the built host calculator}" > "$out/runtime.log" 2>&1 &
app_pid=$!
window=
for i in $(seq 1 80); do window=$(xdotool search --onlyvisible --name '^Calculator$' 2>/dev/null | head -1 || true); [ -z "$window" ] || break; sleep .1; done
[ -n "$window" ]
xdotool windowfocus --sync "$window"
sleep .5
import -window "$window" "$out/initial.png"
xdotool type --window "$window" --delay 60 '128*3.5'
xdotool key --window "$window" Return
sleep .5
import -window "$window" "$out/result.png"
xdotool key --window "$window" ctrl+a
xdotool type --window "$window" --delay 25 '12345*67+89012/34-56789*0.0123+45678/9'
sleep .4
import -window "$window" "$out/long-expression-focused.png"
xdotool key --window "$window" shift+Home
sleep .3
import -window "$window" "$out/long-expression-selection.png"
xdotool key --window "$window" End Return
sleep .3
import -window "$window" "$out/long-expression-result.png"
xdotool key --window "$window" Escape
xdotool type --window "$window" --delay 25 '128*3.5'
xdotool key --window "$window" Return
xdotool windowsize "$window" 340 536
sleep .5
import -window "$window" "$out/minimum.png"
xdotool windowsize "$window" 480 720
sleep .5
import -window "$window" "$out/resized.png"
xdotool windowsize "$window" 380 584
xdotool mousemove --window "$window" 164 20 click 1
sleep .5
import -window "$window" "$out/converter.png"
xdotool mousemove --window "$window" 55 330 click 1
xdotool mousemove --window "$window" 145 400 click 1
sleep .4
import -window "$window" "$out/converter-keypad.png"
xdotool windowsize "$window" 340 536
sleep .5
import -window "$window" "$out/converter-minimum.png"
xdotool mousemove --window "$window" 205 20 click 1
sleep .5
import -window "$window" "$out/history.png"
xdotool mousemove --window "$window" 45 20 click 1
xdotool key --window "$window" Escape
xdotool type --window "$window" --delay 60 '1/0'
xdotool key --window "$window" Return
sleep .5
import -window "$window" "$out/error.png"
kill -0 "$app_pid"
printf 'Actual CPU-rendered screenshots: %s\n' "$out"
