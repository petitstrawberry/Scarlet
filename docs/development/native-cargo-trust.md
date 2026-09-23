# Native Cargo TLS trust on Scarlet

Scarlet supplies one OS-managed PEM root bundle at
`/etc/ssl/certs/ca-certificates.crt`. The base bundle installs it from the
`scarlet-ca-certificates` Nix output, which uses the repository's pinned
`nixpkgs.cacert`. It is included in images composing `bundles/base`, regardless
of whether the experimental native Cargo binary is installed. The image build
copies the file with mode `0644`; it does not embed CA certificates in Cargo.

The Scarlet curl/curl-sys fork sets that path as libcurl's default CA file.
Cargo leaves curl's default CA selection intact, while its normal `cainfo`
setting can still override the path. The Scarlet
`rustls-platform-verifier` fork reads the same file for Rust TLS clients.
The default verifiers have no bundled root-list fallback when that file is
unavailable or empty; an unrelated root fails the crates.io handshake. The
curl fork's libcurl C submodule is
the `petitstrawberry/curl` fork; these are source repositories pinned by Git
revision in the Cargo port rather than edits in a local Cargo cache.

Update trust anchors by updating `flake.lock` to a reviewed nixpkgs revision,
building a new image, and checking the installed bundle fingerprint. Runtime
CA updates and per-user trust controls are not implemented. A writable rootfs
file is not a tamper-resistant store: if Scarlet later supports stronger
isolation or verified boot, the bundle should be signed and installed through
an authenticated, atomic update path, with a trusted verification key outside
the writable rootfs. TLS clients can keep reading the same path.

The AArch64 HVF acceptance runs Cargo inside Scarlet with an empty Cargo home,
fetches `itoa 1.0.15` from crates.io over HTTPS, builds it with the native
compiler, and executes the result. The guest reports
`SCARLET_NATIVE_CARGO_ONLINE_OK=42`; curl's diagnostic reports the OS CA path.
Replacing the staged PEM with an unrelated self-signed root produces
`UnknownIssuer` after DNS and TCP succeed, so an embedded fallback did not
silently authorize the server. The current online fixture builds under guest
`/tmp` because ext2 still rejects removal of rustc's empty temporary archive
directory; this does not establish that a normal Cargo build directory on ext2
works yet. Native Cargo is not in the published toolchain bundle.
