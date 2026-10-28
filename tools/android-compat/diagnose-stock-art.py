#!/usr/bin/env python3
"""Host Linux qemu-user diagnostic of stock ART inputs. NEVER Scarlet evidence.

Uses isolated, network-disabled Docker containers and an explicitly supplied
static qemu-aarch64. Creates property areas and runs the image's linkerconfig
with Android init's arguments, then executes the DEX with -Xint.
"""
import argparse
import base64
import json
from pathlib import Path
import shutil
import socket
import subprocess
import threading
import uuid

from art_inputs import art_launch, sha256, validate_dex, verify_art_runtime
from stock_art import decode_logd, linkerconfig_succeeded, verify_stock_inputs

IMAGE = "nixos/nix@sha256:4bfe741027f0bfbef745862cbf3b5f424ee99801c04138146c0000a28759636d"


def stage(name, argv, env, root, output, qemu, image, timeout):
    if any("," in str(path) for path in (root, output, qemu)):
        raise ValueError("Docker bind source cannot contain a comma")
    sockets = output / "sockets"
    sockets.mkdir(exist_ok=True)
    receiver = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    receiver.bind(str(sockets / "logdw"))
    receiver.settimeout(0.1)
    packets, done = [], threading.Event()

    def collect():
        while True:
            try:
                packets.append(receiver.recv(65536))
            except TimeoutError:
                if done.is_set():
                    break

    reader = threading.Thread(target=collect)
    reader.start()
    container = "scarlet-art-diagnostic-" + uuid.uuid4().hex[:12]
    command = ["docker", "run", "--rm", "--pull", "never", "--name", container,
               "--network", "none", "--ulimit", "core=0"]
    mounts = [(qemu, "/emulator/qemu-aarch64", True), (sockets, "/dev/socket", True),
              (root / "dev/__properties__", "/dev/__properties__", name != "properties")]
    mounts += [(root / directory, "/" + directory, directory not in ("data", "linkerconfig"))
               for directory in ("system", "system_ext", "vendor", "product", "apex", "data", "linkerconfig")]
    for source, target, readonly in mounts:
        command += ["--mount", f"type=bind,src={source},dst={target}" + (",readonly" if readonly else "")]
    for value in env:
        command += ["-e", value]
    command += [image, "/emulator/qemu-aarch64"] + argv
    record = {"platform": "host Linux qemu-user, NOT Scarlet", "argv": command}
    try:
        process = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
        record["returncode"] = process.returncode
        stdout, stderr = process.stdout, process.stderr
    except subprocess.TimeoutExpired as error:
        record["timeout"] = True
        stopped = subprocess.run(["docker", "stop", "-t", "1", container], capture_output=True, text=True, timeout=15)
        record["stop"] = {"returncode": stopped.returncode, "stdout": stopped.stdout, "stderr": stopped.stderr}
        stdout = (error.stdout or b"").decode(errors="replace")
        stderr = (error.stderr or b"").decode(errors="replace")
    finally:
        done.set()
        reader.join()
        receiver.close()
        (sockets / "logdw").unlink()
    record["logd_packets_base64"] = [base64.b64encode(packet).decode() for packet in packets]
    messages = []
    for packet in packets:
        try:
            messages.append(decode_logd(packet))
        except ValueError as error:
            record.setdefault("logd_errors", []).append(str(error))
    record["messages"] = messages
    (output / f"{name}.json").write_text(json.dumps(record, indent=2) + "\n")
    (output / f"{name}.stdout.log").write_text(stdout)
    (output / f"{name}.stderr.log").write_text(stderr)
    (output / f"{name}.logd.log").write_text("".join(f"{m['priority']} {m['tag']}: {m['message']}\n" for m in messages))
    print(f"{name}: exit={record.get('returncode')}, timeout={record.get('timeout', False)}", flush=True)
    return record, stdout


def diagnose(runtime, stock, dex, qemu, output, image=IMAGE, timeout=60):
    base = verify_art_runtime(runtime)
    metadata = verify_stock_inputs(stock, runtime)
    dex_hash = validate_dex(dex)
    output.mkdir(parents=True, exist_ok=False)
    root = output / "root"
    shutil.copytree(runtime, root, symlinks=True)
    for relative in metadata["directories"]:
        (root / relative).mkdir(parents=True, exist_ok=True)
    for relative in metadata["files"]:
        destination = root / relative
        if destination.exists() and sha256(destination) != metadata["files"][relative]:
            raise ValueError(f"stock input would replace a different base file: {relative}")
        shutil.copy2(stock / relative, destination)
    for relative, target in metadata["symlinks"].items():
        (root / relative).symlink_to(target)
    for relative in metadata["symlinks"]:
        path = root / relative
        if not path.resolve().is_relative_to(root) or not path.is_file():
            raise ValueError(f"missing or escaping library alias target: {relative}")
    shutil.copyfile(dex, root / "data/local/tmp/classes.dex")
    record = {"platform": "host Linux qemu-user, NOT Scarlet", "result": "INCOMPLETE",
              "base_manifest_sha256": sha256(runtime / "art-runtime.json"),
              "stock_manifest_sha256": sha256(stock / "stock-inputs.json"),
              "dex_sha256": dex_hash, "qemu_sha256": sha256(qemu),
              "qemu_version": subprocess.check_output([str(qemu), "--version"], text=True), "image": image}
    result_file = output / "result.json"
    result_file.write_text(json.dumps(record, indent=2) + "\n")
    env = ["ANDROID_LOG_TAGS=*:v"]
    properties, stdout = stage("properties", ["/data/local/tmp/property-bootstrap",
                       "/data/local/tmp/property-contexts.list", "/data/local/tmp/property-values.prop"],
                       env, root, output, qemu, image, timeout)
    count = len((root / "data/local/tmp/property-values.prop").read_text().splitlines())
    if properties.get("returncode") != 0 or stdout.splitlines() != [f"SCARLET_PROPERTY_SNAPSHOT_OK {count}"]:
        record["blocker"] = "property initialization"
    else:
        # Match stock init. --strict is a diagnostic option which can emit a
        # partial graph for this image's absent optional debug-library provider.
        config, _ = stage("linkerconfig", ["/apex/com.android.runtime/bin/linkerconfig", "--target", "/linkerconfig"],
                          env, root, output, qemu, image, timeout)
        art_config = root / "linkerconfig/com.android.art/ld.config.txt"
        if (not linkerconfig_succeeded(config.get("returncode"), config["messages"]) or
                config.get("logd_errors") or not art_config.is_file() or
                "namespace.system.visible = true" not in art_config.read_text()):
            record["blocker"] = "stock linkerconfig generation"
        else:
            record["generated_files"] = {str(path.relative_to(root)): sha256(path)
                for directory in (root / "linkerconfig", root / "dev/__properties__")
                for path in sorted(directory.rglob("*")) if path.is_file()}
            launch = art_launch(base)
            # The generated namespaces supply the search paths and links.
            launch["env"] = [value for value in launch["env"] if not value.startswith("LD_LIBRARY_PATH=")]
            art, stdout = stage("art", launch["argv"], launch["env"], root, output, qemu, image, timeout)
            expected = ["SCARLET_ART_BEGIN", "Scarlet ART: 42", "SCARLET_ART_OK"]
            if art.get("returncode") == 0 and stdout.splitlines() == expected:
                record["result"] = "HOST_LINUX_PASS"
            else:
                record["blocker"] = "ART startup/execution; see art logs"
    result_file.write_text(json.dumps(record, indent=2) + "\n")
    print(record["result"] + " (host Linux only; no Scarlet verification)")
    return record["result"] == "HOST_LINUX_PASS"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--stock-inputs", type=Path, required=True)
    parser.add_argument("--dex", type=Path, required=True)
    parser.add_argument("--qemu-user", type=Path, required=True, help="static host qemu-aarch64 executable")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--image", default=IMAGE)
    parser.add_argument("--timeout", type=int, default=60, help="seconds per stage")
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    success = diagnose(args.runtime_root.resolve(), args.stock_inputs.resolve(), args.dex.resolve(),
                       args.qemu_user.resolve(), args.output.resolve(), args.image, args.timeout)
    raise SystemExit(0 if success else 1)


if __name__ == "__main__":
    main()
