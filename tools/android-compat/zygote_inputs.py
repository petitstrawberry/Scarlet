"""Stock Zygote control-socket readiness fixture; no Android runtime patches."""
import json
from pathlib import Path
import re

from art_inputs import sha256
from framework_inputs import framework_launch

SOURCE = Path(__file__).resolve().parent


def launch(metadata):
    value = framework_launch(metadata)
    value["argv"] = value["argv"][:8] + ["--zygote", "--abi-list=arm64-v8a", "--socket-name=zygote", "--enable-lazy-preload"]
    value["argv"].insert(1, "-Xzygote")
    value["env"] = [item for item in value["env"] if not item.startswith("CLASSPATH=")]
    return value


def header(metadata):
    value = launch(metadata)
    return "".join("static const char *zygote_" + key + "[]={" +
                   ",".join(json.dumps(s) for s in strings) + ",NULL};\n"
                   for key, strings in (("environment", value["env"]), ("argv", value["argv"])))


def verify_build(build, metadata, framework):
    record = json.loads((build / "build.json").read_text())
    if (record.get("kind") != "stock-zygote-control-v1"
            or record.get("source_sha256") != sha256(SOURCE / "zygote-probe.c")
            or record.get("framework_sha256") != sha256(framework / "framework-inputs.json")
            or record.get("program_sha256") != sha256(build / "zygote-probe")
            or (build / "zygote-config.h").read_text() != header(metadata)):
        raise ValueError("changed or mismatched Zygote fixture")
    return build / "zygote-probe"


def analyze(data):
    lines = data.decode("utf-8", "replace").replace("\r", "").splitlines()
    events = [line for line in lines if line.startswith("SCARLET_ZYGOTE_PROBE_")]
    spawn = re.fullmatch(r"SCARLET_ZYGOTE_PROBE_SPAWN ([1-9][0-9]*)", events[0]) if events else None
    pid = int(spawn[1]) if spawn else None
    expected = [f"SCARLET_ZYGOTE_PROBE_SPAWN {pid}", "SCARLET_ZYGOTE_PROBE_ABI_OK arm64-v8a",
                f"SCARLET_ZYGOTE_PROBE_PID_OK {pid}", "SCARLET_ZYGOTE_PROBE_REAP 9", "SCARLET_ZYGOTE_PROBE_OK"]
    passed = (pid is not None and events == expected
              and lines.count("SCARLET_ANDROID_PROGRAM_EXIT 0") == 1
              and lines.count("SCARLET_ANDROID_STOCK_OK") == 1
              and lines.count("SCARLET_ANDROID_PROGRAM_DONE") == 1)
    return {"result": "PASS" if passed else "INCOMPLETE", "pid": pid,
            "queries": ["--query-abi-list", "--get-pid"] if passed else [],
            "child_specialization": "not tested", "activity_launch": "not tested"}
