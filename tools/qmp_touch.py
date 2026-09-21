#!/usr/bin/env python3
"""Inject a tap, swipe, or two-finger pinch into a QEMU multitouch device."""

import argparse
import math
import os
import time

from qmp import Qmp


ABS_MAX = 0x7FFF


def unit(value):
    result = float(value)
    if not math.isfinite(result) or not 0 <= result <= 1:
        raise argparse.ArgumentTypeError("coordinates and radii must be between 0 and 1")
    return result


def position(value):
    return round(value * ABS_MAX)


def contact_events(kind, slot, x, y):
    tracking_id = -1 if kind == "end" else slot
    events = [{"type": "mtt", "data": {
        "type": kind, "slot": slot, "tracking-id": tracking_id,
        "axis": "x", "value": 0,
    }}]
    if kind != "end":
        for axis, coordinate in (("x", x), ("y", y)):
            events.append({"type": "mtt", "data": {
                "type": "data", "slot": slot, "tracking-id": tracking_id,
                "axis": axis, "value": position(coordinate),
            }})
    return events


def frame(qmp, kind, contacts):
    events = []
    for slot, x, y in contacts:
        events.extend(contact_events(kind, slot, x, y))
    events.append({"type": "btn", "data": {
        "button": "touch", "down": kind != "end",
    }})
    qmp.execute("input-send-event", {"events": events})


def gesture(qmp, start, finish, duration, steps):
    frame(qmp, "begin", start)
    try:
        if finish is not None:
            for step in range(1, steps + 1):
                fraction = step / steps
                contacts = [
                    (slot, x + (end_x - x) * fraction, y + (end_y - y) * fraction)
                    for (slot, x, y), (_, end_x, end_y) in zip(start, finish)
                ]
                time.sleep(duration / steps)
                frame(qmp, "update", contacts)
        else:
            time.sleep(duration)
    finally:
        frame(qmp, "end", start)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default=os.environ.get("SCARLET_QEMU_QMP", "/tmp/scarlet-input-qmp.sock"))
    commands = parser.add_subparsers(dest="command", required=True)

    tap = commands.add_parser("tap", help="one touch at normalized x,y")
    tap.add_argument("x", type=unit)
    tap.add_argument("y", type=unit)

    swipe = commands.add_parser("swipe", help="one contact moving from x1,y1 to x2,y2")
    for name in ("x1", "y1", "x2", "y2"):
        swipe.add_argument(name, type=unit)

    pinch = commands.add_parser("pinch", help="two contacts moving horizontally around x,y")
    for name in ("x", "y", "start_radius", "end_radius"):
        pinch.add_argument(name, type=unit)

    for command in (tap, swipe, pinch):
        command.add_argument("--duration", type=float, default=0.12 if command is tap else 0.5)
    for command in (swipe, pinch):
        command.add_argument("--steps", type=int, default=12)

    args = parser.parse_args()
    if not math.isfinite(args.duration) or not 0.02 <= args.duration <= 10:
        parser.error("duration must be between 0.02 and 10 seconds")
    if hasattr(args, "steps") and not 1 <= args.steps <= 120:
        parser.error("steps must be between 1 and 120")

    if args.command == "tap":
        start, finish = [(0, args.x, args.y)], None
    elif args.command == "swipe":
        start = [(0, args.x1, args.y1)]
        finish = [(0, args.x2, args.y2)]
    else:
        radius = max(args.start_radius, args.end_radius)
        if not (0 <= args.x - radius and args.x + radius <= 1):
            parser.error("pinch contacts must remain inside the display")
        start = [(0, args.x - args.start_radius, args.y), (1, args.x + args.start_radius, args.y)]
        finish = [(0, args.x - args.end_radius, args.y), (1, args.x + args.end_radius, args.y)]

    try:
        with Qmp(args.socket) as qmp:
            gesture(qmp, start, finish, args.duration, getattr(args, "steps", 1))
    except (OSError, EOFError, TimeoutError, ValueError, RuntimeError) as error:
        parser.exit(1, f"{error}\n")
    print(f"Sent {args.command} to {args.socket}")


if __name__ == "__main__":
    main()
