#!/usr/bin/env python3
"""Stage existing native binaries and only temporary document fixtures."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

REPO = Path(__file__).resolve().parents[2]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def provenance(root):
    paths = subprocess.check_output(
        ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard"],
        text=True,
    ).splitlines()
    return {
        "head": subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip(),
        "status": subprocess.check_output(["git", "-C", str(root), "status", "--short"], text=True),
        "source_sha256": {
            path: digest(root / path) for path in sorted(set(paths))
            if Path(path).suffix in {".rs", ".toml", ".lock", ".py", ".sh"}
            and (root / path).is_file()
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--init", type=Path, required=True)
    parser.add_argument("--sbusd", type=Path, required=True)
    parser.add_argument("--prepare", type=Path, required=True)
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--descriptor", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True, help="directory containing fonts/ and cursors/")
    parser.add_argument("--ui-source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="new directory; existing directories are refused")
    parser.add_argument("--app", choices=["notepad", "files", "task-manager", "screenshot"], default="notepad")
    parser.add_argument("--build-label", required=True, help="published pins or explicit local patch description")
    args = parser.parse_args()
    staging = args.output.resolve()
    binaries = ["scarlet-ld", "stemd", "logd", "sws", "scarlet-shell", "files", "notepad", "task-manager", "screenshot"]
    fonts = ["Mplus1-Regular.ttf", "JetBrainsMonoNerdFontMono-Regular.ttf"]
    required = [args.init, args.sbusd, args.prepare, args.driver, args.descriptor]
    required += [args.bin_dir / name for name in binaries]
    required += [args.assets / "fonts" / name for name in fonts]
    missing = [str(path) for path in required if not path.is_file()]
    if not (args.assets / "cursors/default").is_dir():
        missing.append(str(args.assets / "cursors/default"))
    if not (args.assets / "icons/default").is_dir():
        missing.append(str(args.assets / "icons/default"))
    if missing:
        parser.error("Build or provide missing inputs first: " + ", ".join(missing))
    staging.mkdir(parents=True, exist_ok=False)
    for relative in ["bin", "lib/sgfx", "root", "tmp", "dev/pts", "mnt/newroot", "etc/sws", "etc/stemd.d/services", "share/fonts", "share/cursors"]:
        (staging / relative).mkdir(parents=True, exist_ok=True)
    inputs = {}

    def copy(source, relative):
        shutil.copy2(source, staging / relative)
        inputs[relative] = {"source": str(source.resolve()), "sha256": digest(staging / relative)}

    copy(args.init, "init")
    copy(args.sbusd, "bin/sbusd")
    copy(args.prepare, "bin/prepare-desktop")
    for name in binaries:
        copy(args.bin_dir / name, f"bin/{name}")
    copy(args.driver, "lib/sgfx/libsgfx_scarlet_virgl.so")
    copy(args.descriptor, "lib/sgfx/scarlet-virgl.sgfx-driver")
    for name in fonts:
        copy(args.assets / "fonts" / name, f"share/fonts/{name}")
    shutil.copytree(args.assets / "cursors" / "default", staging / "share/cursors/default")
    shutil.copytree(args.assets / "icons" / "default", staging / "share/icons/default")
    (staging / "root/fixture.txt").write_text(
        "Native desktop smoke\n\nThis document is a temporary fixture.\n"
        "Text editing, selection, save and dialogs should remain responsive.\n"
    )
    (staging / "etc/sws/config.toml").write_text('[output]\nscale = 1.0\n[cursor]\ntheme = "/share/cursors/default"\n')
    command = {"notepad": "/bin/notepad /tmp/fixture.txt", "files": "/bin/files /tmp",
               "task-manager": "/bin/task-manager", "screenshot": "/bin/screenshot"}[args.app]
    (staging / "etc/stemd.d/services/00-smoke.toml").write_text(
        '[service.logd]\nexec = "/bin/logd"\nready_notify = true\nready_timeout_ms = 10000\n'
        '[service.sbusd]\nexec = "/bin/sbusd"\n'
        '[service.stemd]\nexec = "/bin/stemd"\ndepends = ["logd", "sbusd"]\n'
        '[service.sws]\nexec = "/bin/sws"\ndepends = ["sbusd"]\nready_notify = true\nready_timeout_ms = 30000\n'
        '[service.scarlet-shell]\nexec = "/bin/scarlet-shell --mode desktop"\ndepends = ["sws"]\n'
        '[service.desktop-files]\nexec = "/bin/files --service"\ndepends = ["sws"]\n'
        f'[service.smoke-app]\nexec = "{command}"\ndepends = ["sws", "scarlet-shell"]\n'
    )
    manifest = {"build_label": args.build_label, "app": args.app, "inputs": inputs,
                "scarlet": provenance(REPO), "scarlet_ui": provenance(args.ui_source.resolve())}
    (staging.parent / f"{staging.name}-provenance.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Staged {sum(p.stat().st_size for p in staging.rglob('*') if p.is_file())} bytes in {staging}")


if __name__ == "__main__":
    main()
