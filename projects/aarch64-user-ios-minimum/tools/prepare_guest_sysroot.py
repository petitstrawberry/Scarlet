"""Build the Scarlet Rust sysroot with x18 reserved, using the pinned compiler."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    source = Path(__file__).resolve()
    cache = source.parent.parent / ".scarlet" / "cache" / "guest-sysroots"
    cache.mkdir(parents=True, exist_ok=True)
    target = "aarch64-unknown-scarlet"
    rustc = os.environ.get("RUSTC", "rustc")
    version = subprocess.check_output([rustc, "-vV"])
    original = Path(subprocess.check_output(
        [rustc, "--print", "target-libdir", "--target", target],
        text=True,
    ).strip())
    key = hashlib.sha256(version + os.fsencode(original) + source.read_bytes()).hexdigest()
    sysroot = cache / key
    if not sysroot.exists():
        with tempfile.TemporaryDirectory(dir=cache) as work:
            build = Path(work)
            (build / "src").mkdir()
            (build / "src/main.rs").write_text("fn main() {}\n")
            (build / "Cargo.toml").write_text(
                '[package]\nname = "scarlet-guest-sysroot"\n'
                'version = "0.0.0"\nedition = "2024"\n'
                '[profile.release]\npanic = "abort"\n'
            )
            env = os.environ.copy()
            env.pop("CARGO_ENCODED_RUSTFLAGS", None)
            env["RUSTFLAGS"] = "-Zfixed-x18 -C relocation-model=pic"
            subprocess.run(
                ["cargo", "build", "--release", "--target", target,
                 "--target-dir", build / "target", "-Zbuild-std=std,panic_abort",
                 "-Zbuild-std-features=compiler-builtins-mem"],
                cwd=build, env=env, check=True,
            )
            staged = build / "sysroot"
            library = staged / "lib" / "rustlib" / target / "lib"
            library.mkdir(parents=True)
            for rlib in (build / "target" / target / "release" / "deps").glob("*.rlib"):
                shutil.copy2(rlib, library)
            shutil.copy2(original / "scarlet-crt0.o", library)
            try:
                staged.rename(sysroot)
            except OSError:
                if not sysroot.is_dir():
                    raise
    print(sysroot)


if __name__ == "__main__":
    main()
