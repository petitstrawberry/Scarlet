#!/usr/bin/env bash
# Run on a dedicated X display; every application action uses in-memory fixtures.
set -euo pipefail
binary=${1:?usage: visual_smoke.sh BINARY OUTPUT_DIRECTORY [--empty]}
output=${2:?usage: visual_smoke.sh BINARY OUTPUT_DIRECTORY [--empty]}
fixture_args=()
if [[ ${3:-} == --empty ]]; then fixture_args+=(--empty); fi
mkdir -p "$output"
"$binary" "${fixture_args[@]}" > "$output/runtime.log" 2>&1 &
app_pid=$!
trap 'kill "$app_pid" 2>/dev/null || true; wait "$app_pid" 2>/dev/null || true' EXIT
window=''
for attempt in $(seq 1 100); do
    kill -0 "$app_pid"
    window=$(xdotool search --onlyvisible --name 'Settings.*fixture preview' 2>/dev/null | head -1 || true)
    [[ -z "$window" ]] || break
    sleep .1
done
[[ -n "$window" ]]
xdotool windowfocus --sync "$window"
capture() {
    sleep .3
    local candidate="$output/.$1.pending.png"
    local deadline=$((SECONDS + 10))
    # Winit can expose its X window before SGFX submits the first frame. Do not
    # accept the uniform black startup surface as visual validation evidence.
    while (( SECONDS < deadline )); do
        kill -0 "$app_pid"
        if timeout 2s import -window "$window" "$candidate" \
            && [[ $(identify -format '%[fx:standard_deviation > 0]' "$candidate") == 1 ]]; then
            mv "$candidate" "$output/$1.png"
            return
        fi
        sleep .1
    done
    rm -f "$candidate"
    echo "Timed out waiting for a rendered Settings frame: $1" >&2
    return 1
}
capture appearance
for item in 'display 95' 'cursor 135' 'audio 175' 'input 215' 'status 255' 'about 295'; do
    read -r name y <<< "$item"
    xdotool mousemove --window "$window" 60 "$y" click 1
    capture "$name"
done
xdotool windowsize "$window" 800 574
capture about-minimum
xdotool mousemove --window "$window" 60 55 click 1
xdotool mousemove --window "$window" 730 480 click --repeat 30 --delay 30 5
capture appearance-bottom
xdotool mousemove --window "$window" 60 255 click 1
xdotool mousemove --window "$window" 730 480 click --repeat 30 --delay 30 5
capture status-bottom
xdotool windowsize "$window" 1060 760
xdotool mousemove --window "$window" 60 175 click 1
capture audio-large
if [[ ${3:-} != --empty ]]; then
    xdotool windowsize "$window" 800 634
    sleep .3
    xdotool mousemove --window "$window" 450 176 click 1
    xdotool key --window "$window" Down Return
    capture audio-selection
    xdotool key --window "$window" Tab Tab Tab space
    capture audio-focus-mute
fi
kill "$app_pid"
wait "$app_pid" 2>/dev/null || true
# Restart to make the initial keyboard focus independent of pointer interactions.
"$binary" "${fixture_args[@]}" >> "$output/runtime.log" 2>&1 &
app_pid=$!
window=''
for attempt in $(seq 1 100); do
    kill -0 "$app_pid"
    window=$(xdotool search --onlyvisible --name 'Settings.*fixture preview' 2>/dev/null | head -1 || true)
    [[ -z "$window" ]] || break
    sleep .1
done
[[ -n "$window" ]]
xdotool windowfocus --sync "$window"
xdotool windowsize "$window" 800 574
sleep .3
xdotool key --window "$window" Tab
capture appearance-keyboard-start
xdotool key --window "$window" Page_Down Page_Down Page_Down shift+Tab
capture appearance-keyboard-action-focus
xdotool key --window "$window" Return
capture appearance-keyboard-save
xdotool key --window "$window" Home
capture appearance-keyboard-home
