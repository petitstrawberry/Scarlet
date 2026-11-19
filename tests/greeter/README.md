The host tests mount the production login View at 320×360, 800×480, 1280×720
and 600×1024. They check the trusted hit targets against visible controls and
exercise raw input normalization, field selection, physical key translation
and the bounded greeter protocol. Run from the repository root:

```sh
cargo test --locked --manifest-path tests/greeter/Cargo.toml --test interaction
cargo run --locked --manifest-path tests/greeter/Cargo.toml --bin greeter-fixture -- \
  bundles/desktop/fs/share/fonts/Mplus1-Regular.ttf 800 480 /tmp/greeter.ppm
```

Append `picker` to render the account chooser or `failed` to display the authentication error.
For guest acceptance, boot the desktop image with a configured local test
account and a direct touchscreen or VirtIO tablet. Use only taps to select
an account, submit an incorrect password, then enter the
correct password and verify that the configured desktop starts. Repeat with
physical keys (including Shift, Backspace and Enter), in portrait and landscape.
