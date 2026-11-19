The host tests mount the production ScarletUI login form at 320×360, 800×480,
1280×720 and 600×1024. They cover account selection, submission, keyboard
events, masked surrounding text, and field placement above the reported panel
occlusion. The clock and account actions share a centered stack on the chooser. The
clock yields to the text editor during entry. Protocol tests check the two
restricted SWS client roles.

Run from the repository root:

```sh
cargo test --locked --manifest-path tests/greeter/Cargo.toml --test interaction --test system-panel
cargo run --locked --manifest-path tests/greeter/Cargo.toml --bin greeter-fixture -- \
  bundles/desktop/fs/share/fonts/Mplus1-Regular.ttf 800 480 /tmp/greeter.ppm
```

Append `picker` to render the account chooser or `failed` to display the authentication error.
The chooser fixture uses 09:41 as a reproducible sample time.
An optional final argument reserves the supplied number of logical pixels at the
bottom of the form. These snapshots show only the form. The system input panel
runs in its own process in the guest.

For guest acceptance, boot the desktop image with a configured local test
account and a direct touchscreen or VirtIO tablet. Use only taps to select
an account, submit an incorrect password, then enter the
correct password and verify that the configured desktop starts. Repeat with
physical keys (including Shift, Backspace and Enter), in portrait and landscape.
