# Wayland keyboard map

`us.xkb` is the self-contained evdev/pc105 US map serialized as
XKB_KEYMAP_FORMAT_TEXT_V1 by libxkbcommon from Debian trixie xkb-data.
It uses Linux input keycodes plus the XKB offset of 8, matching the SWS seat.
Unlike a map containing include directives, clients can load it without any
local XKB data files. The source package notices are in COPYRIGHT.

Regenerate with xkb_context_new, xkb_keymap_new_from_names using
rules=evdev, model=pc105, layout=us, options="", then
xkb_keymap_get_as_string(map, XKB_KEYMAP_FORMAT_TEXT_V1).
The map currently describes a US physical layout; other layouts need a
separate selection mechanism.
