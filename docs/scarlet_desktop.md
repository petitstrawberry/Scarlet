# Scarlet Desktop (SWS clients)

Scarlet Desktop is assembled from **regular SWS clients**. The compositor (SWS server) is responsible for stacking and compositing surfaces, while desktop components are implemented as separate programs.

## Components

- `scarlet_desktop` (session launcher)
  - Starts desktop components as separate processes.
  - Waits for child exits and keeps the session alive.

- `scarlet_desktop_background` (desktop background)
  - Creates an SWS surface and sets `window_types::DESKTOP`.
  - Requests maximize to learn the screen size (via `WINDOW_CONFIGURE`).
  - Resizes its surface to full screen and draws a simple gradient background.

- `scarlet_desktop_taskbar` (taskbar)
  - Creates an SWS surface and sets `window_types::TASKBAR`.
  - Requests maximize to learn the screen size (via `WINDOW_CONFIGURE`).
  - Resizes to `screen_width x BAR_HEIGHT` and moves itself to the bottom.
  - Handles basic input:
    - Tracks `EV_ABS/ABS_X` and `EV_ABS/ABS_Y` for pointer coordinates.
    - Tracks `EV_KEY/BTN_LEFT` to detect clicks.
  - Clicking the "Overview" button launches `scarlet_desktop_overview`.

- `scarlet_desktop_overview` (overview window)
  - A regular UI window rendered with ScarletUI widgets.
  - Currently acts as a demo surface and a convenient target for taskbar launching.

## Window types / stacking

SWS defines window types in `sws_protocol::window_types`:

- `DESKTOP` (3): lowest layer (background)
- `TASKBAR` (2): above desktop, below normal windows
- `ALWAYS_ON_TOP` (1)
- `NORMAL` (0)

The compositor uses these types to keep a consistent Z-order.

## Running

Build as usual (example):

- `cargo make build-riscv64`

Then launch SWS and the desktop session from your init/login flow:

- Start SWS (`sws`)
- Start `scarlet_desktop`

The session will spawn the background and taskbar components.
