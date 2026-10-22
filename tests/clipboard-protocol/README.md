# Native text clipboard checks

Run the production protocol/client and session-store tests without compositor
hardware or unrelated networking/media application dependencies (the client is
native-only and is checked by the cross-build below):

```sh
cargo test --manifest-path tests/clipboard-protocol/Cargo.toml
```

The native SWS service keeps one UTF-8 string (at most 256 KiB) in server memory
for the session. Every read/write runs on the compositor thread and checks
canonical focus, presentation/visibility and window ownership. Hidden or minimized
windows cannot reuse stale focus to read or replace text. Queuing reserves a slot
before copying a payload, with at most one pending request per client and 16 total.
Closing the copying application does not erase the session copy. Empty clipboard
and a copied empty string are distinct. Invalid UTF-8, oversized payloads and
trailing bytes fail validation; rejected writes preserve the previous value.

Protocol version 14 advertises `TEXT_CLIPBOARD` (bit 17). Requests 57/58 carry a
window ID and, for Set, a u32 byte length followed by UTF-8. Response 43 contains
status and u32 byte length followed by UTF-8. Status 0 means success, 1 means
access denied, 2 means no text and 3 means the queue is busy. Non-success responses contain no text.
Clients check the capability before requesting and preserve request correlation.

These are protocol/model tests, not a live SWS cross-application demonstration.
The ScarletUI service runs bounded client IPC on a worker thread and reports
completion/errors. Unsupported host backends report Unsupported explicitly;
they never replace the desktop clipboard with an application buffer.

The optional `server` feature compiles the production SWS daemon source against
its actual dependencies without unrelated media/networking application binaries:

```sh
cargo check --manifest-path tests/clipboard-protocol/Cargo.toml --target aarch64-unknown-scarlet --features server --bin sws
```

These commands use the published dependency pins. This is a
cross-compilation check; it does not boot SWS or prove cross-application clipboard
delivery on a running Scarlet session.

Validation on 2026-10-22: 4 protocol/session/queue tests and 6 existing wire-contract
tests passed (`/tmp/clipboard-protocol-tests.log`). The current Scarlet toolchain
cross-check of the complete daemon passed (`/tmp/clipboard-sws-current-native-check.log`).
The UI service's two worker regressions and two existing text clipboard callback
tests passed with the following command (`/tmp/clipboard-core-final-tests.log`):

```sh
cargo test -p scarlet-ui-core clipboard -- --test-threads=1
```
