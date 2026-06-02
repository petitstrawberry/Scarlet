# Scarlet SKK

`scarlet_skk` is an external SKK input method service for exercising the SWS text-input protocol. It keeps conversion logic outside SWS; SWS only brokers text-input state, trigger delivery, key arbitration, preedit, commit, deletion, and input-method-owned popup placement.

## Behavior

- Registers as `scarlet-skk` and requests active IME status.
- Lower-case romaji commits kana directly.
- `Shift` + letter starts `▽` midashi input.
- `Shift` + letter inside midashi starts an okuri marker.
- Printable number and symbol keys are interpreted by the IME itself as fullwidth characters while composing.
- `Space` enters or advances `▼` candidate selection.
- `Backspace` moves backward through candidates while `▼` conversion is active.
- `Enter` commits.
- `Esc` cancels or returns to midashi input.

Candidate mode first shows only the inline `▼` preedit. After the user advances or moves candidates, `scarlet_skk` creates an `IME_POPUP` window, renders the visible SKK candidates itself, and asks SWS to anchor that window to the active text-input cursor rectangle with `IME_SET_POPUP_WINDOW`.

## Dictionary

`scarlet_skk` loads a UTF-8 SKK dictionary from these paths:

- `/share/skk/SKK-JISYO.L`
- `/usr/share/skk/SKK-JISYO.L`
- `/usr/local/share/skk/SKK-JISYO.L`
- `/etc/skk/SKK-JISYO.L`

If no usable file exists, it falls back to a tiny built-in dictionary.

`tools/fetch_skk_dictionary.sh` downloads the upstream SKK dictionary and converts it from EUC-JP to UTF-8 for the Scarlet rootfs.
