# Box64 and Wine on the AArch64 Linux ABI

The `full-debian` image uses the `wine` profile from
[scarlet-bundle-debian](https://github.com/petitstrawberry/scarlet-bundle-debian).
It contains native AArch64 Box64 and Debian's amd64 Wine64 packages in the
`linux-aarch64` view. The initial target is 64-bit Windows console programs.
Wine32/Box86/WoW64, GUI, audio and GPU integration are outside this first
bring-up. Native Linux CI success does not establish Scarlet ABI compatibility.
The v0.2.0 Linux console smoke passes and exits normally. Multimedia
initialization still reports unresolved OpenCL, OpenMP and Zstd wrapper symbols;
media playback and full native-library coverage remain open work.

The producer builds and checks the archive in GitHub Actions. Build and boot
the Scarlet image using the existing `aarch64-limine-full` project workflow.

## Probes

Run these individually, in order. `abi-run` opens an ELF image directly, so
invoke the Wine shell launchers through `/bin/sh`:

```sh
abi-run linux-aarch64 /usr/local/bin/box64 --version
abi-run linux-aarch64 /bin/sh /usr/local/bin/wine --version
abi-run linux-aarch64 /bin/sh /usr/local/bin/wineserver --version
abi-run linux-aarch64 /usr/bin/env WINEDLLOVERRIDES=mscoree,mshtml= /bin/sh /usr/local/bin/wine cmd /c ver
```

The first command tests native Box64 startup. The next two execute x86-64 Linux
programs through Box64. The final command initializes `~/.wine` on first use and
exercises Windows PE loading and Wine's server/process/thread/IPC paths. The
override disables Mono/Gecko loading and download prompts for this probe.

Then try a 64-bit Windows executable placed in the shared directory:

```sh
abi-run linux-aarch64 /bin/sh /usr/local/bin/wine /shared/hello.exe
```

For a separate test prefix, add `WINEPREFIX=/tmp/scarlet-wine-test` after
`/usr/bin/env`. To stop the server for that prefix:

```sh
abi-run linux-aarch64 /usr/bin/env WINEPREFIX=/tmp/scarlet-wine-test /bin/sh /usr/local/bin/wineserver -k
```

## Diagnosing a failure

First record which of the probes fails and its complete output. To compare
with the Box64 interpreter, use:

```sh
abi-run linux-aarch64 /usr/bin/env BOX64_DYNAREC=0 BOX64_LOG=1 WINEDLLOVERRIDES=mscoree,mshtml= /bin/sh /usr/local/bin/wine cmd /c ver
```

The ordinary launch uses ARM64 dynarec. The interpreter comparison can help
separate generated-code issues from shared loader, syscall and library paths;
it does not by itself identify the missing ABI behavior.

The launchers call `/usr/local/bin/box64` explicitly with
`/usr/lib/wine/wine64` or `/usr/lib/wine/wineserver64`. No x86-64 kernel ABI or
binfmt_misc registration is needed. Dependencies of both architectures remain
managed by Debian's dpkg database.
Wine's adjacent `/usr/lib/wine/wineserver` selector also points to the Box64
launcher. The original Debian script is preserved as `wineserver.debian` with
a dpkg diversion; this prevents its native shell from directly executing an
amd64 ELF outside Box64.

## Sources and notices

The producer publishes the binary archive together with exact Debian source
packages for both architectures, package notices and its build scripts. The
Box64 build is commit/hash pinned; `/usr/share/doc/box64/` includes its license,
build provenance and build input source archive with per-file notices.
Upstream's prebuilt library and bash/test binaries are excluded. See the
producer's `ATTRIBUTION.md` and release assets for the corresponding sources.
