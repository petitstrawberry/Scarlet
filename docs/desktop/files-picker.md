# Files picker filters

Files owns `org.scarlet-os.desktop.filemanager` on sbus, at
`/org/scarlet/os/filemanager`, interface `org.scarlet.desktop.FileManager`.

`GetPickerCapabilities()` returns String arguments naming supported features.
`extension-list-v1` means the provider enforces the extension filter described
below for both Open and Save. The query does not launch a picker. Older Files
replies with UnknownMethod; clients must not silently assume extension support.

The existing picker methods retain their argument order:

- `OpenFile(title, initial_folder, filter, allow_multiple, select_directories)`
- `SaveFile(title, initial_folder, suggested_name, filter)`

`filter` may be `extensions:wav,flac,json`: a nonempty comma-separated union of
literal extensions, without dots, spaces or wildcards. Each extension consists
of ASCII letters, digits, hyphens or underscores. Matching the final filename
extension is case-insensitive, so `session.resonara.JSON` matches `json` but
`sound.wav.bak` does not match `wav`. Malformed extension lists return
`org.scarlet.desktop.FileManager.InvalidFilter` before any picker is created.

Folders remain visible for navigation. The Open action revalidates the selected
file; Save validates the entered basename against the same filter. Invalid
names keep the picker open and show an error. No extension is silently appended.
Directory-selection requests still select folders instead of files. The normal
Files browser is unfiltered. Existing fixed MIME filters remain supported with
their original semantics, including video player's `video/*`.

Methods return `[String request_id]` immediately. Completion emits `Response`
with `[String request_id, Boolean success, String path]`. Clients correlate the
request ID and validate selected paths/formats before doing application I/O.
Multiple selection, remote cancellation and save overwrite confirmation are
unchanged and are not provided by this extension.

The filter matcher and acceptance rules can be tested on a host without Scarlet
IPC or graphics:

```sh
rustc --edition 2024 --test user/std-bin/src/picker_filter.rs -o /tmp/picker-filter-tests
/tmp/picker-filter-tests
```
