# Scarlet patches to Sunset 0.4.0

This directory vendors Sunset 0.4.0 from upstream tag `sunset-0.4.0`
(`78ced9a736b0bf1d366f76f1652606f0b3f6fc01`). The upstream license is in
`LICENSE`.

Scarlet carries these focused changes:

- import `alloc::boxed::Box` and `alloc::vec` so the existing `rsa` feature
  builds in a `no_std + alloc` target;
- expose server-side PTY request details and `window-change` events;
- expose server APIs for `exit-status`, channel EOF, and channel close;
- keep SSH channel EOF directional instead of echoing an incoming EOF.

The patches are intentionally confined to Sunset's channel, event, runner,
and `alloc` setup code so a future upstream update can replace this vendored
copy cleanly.
