"""Stock Zygote control-socket readiness fixture; no Android runtime patches."""
import json
from pathlib import Path
import re

from art_inputs import sha256, validate_dex
from framework_inputs import framework_launch

SOURCE = Path(__file__).resolve().parent


def launch(metadata, child=False):
    value = framework_launch(metadata)
    value["argv"] = value["argv"][:8] + ["--zygote", "--abi-list=arm64-v8a", "--socket-name=zygote", "--enable-lazy-preload"]
    value["argv"].insert(1, "-Xzygote")
    value["env"] = [item for item in value["env"] if not item.startswith("CLASSPATH=")]
    if child:
        value["env"] = [e + ":/data/local/tmp/zygote-child.dex" if e.startswith("BOOTCLASSPATH=") else e for e in value["env"]]
    return value


def header(metadata, child=False):
    value = launch(metadata, child)
    return ("#define SCARLET_ZYGOTE_CHILD 1\n" if child else "") + "".join("static const char *zygote_" + key + "[]={" +
                   ",".join(json.dumps(s) for s in strings) + ",NULL};\n"
                   for key, strings in (("environment", value["env"]), ("argv", value["argv"])))


def verify_build(build, metadata, framework):
    record = json.loads((build / "build.json").read_text())
    child = record.get("kind") == "stock-zygote-child-v1"
    if (record.get("kind") not in ("stock-zygote-control-v2", "stock-zygote-child-v1")
            or record.get("source_sha256") != sha256(SOURCE / "zygote-probe.c")
            or record.get("framework_sha256") != sha256(framework / "framework-inputs.json")
            or record.get("program_sha256") != sha256(build / "zygote-probe")
            or (build / "zygote-config.h").read_text() != header(metadata, child)):
        raise ValueError("changed or mismatched Zygote fixture")
    if child and (record.get("child_source_sha256") != sha256(SOURCE / "framework/ScarletZygoteChild.java")
                  or record.get("child_dex_sha256") != validate_dex(build / "classes.dex")):
        raise ValueError("changed or mismatched Zygote child fixture")
    return build / "zygote-probe"


def analyze(data, child=False):
    lines = data.decode("utf-8", "replace").replace("\r", "").splitlines()
    events = [line for line in lines if line.startswith("SCARLET_ZYGOTE_PROBE_")]
    spawn = re.fullmatch(r"SCARLET_ZYGOTE_PROBE_SPAWN ([1-9][0-9]*)", events[0]) if events else None
    pid = int(spawn[1]) if spawn else None
    expected = [f"SCARLET_ZYGOTE_PROBE_SPAWN {pid}", "SCARLET_ZYGOTE_PROBE_ABI_OK arm64-v8a",
                f"SCARLET_ZYGOTE_PROBE_PID_OK {pid}", "SCARLET_ZYGOTE_PROBE_SHIELD_OK", "SCARLET_ZYGOTE_PROBE_REAP 9", "SCARLET_ZYGOTE_PROBE_OK"]
    child_pid = None
    if child:
        records = [line for line in events if line.startswith("SCARLET_ZYGOTE_PROBE_CHILD_IDENTITY_OK ")]
        identity = re.fullmatch(r"SCARLET_ZYGOTE_PROBE_CHILD_IDENTITY_OK ([1-9][0-9]*) 10001 10001", records[0]) if len(records) == 1 else None
        child_pid = int(identity[1]) if identity else None
        expected[3:3] = [f"SCARLET_ZYGOTE_PROBE_CHILD_IDENTITY_OK {child_pid} 10001 10001",
            "SCARLET_ZYGOTE_PROBE_CHILD_LABEL_OK u:r:untrusted_app:s0:c1,c256,c512,c768",
            "SCARLET_ZYGOTE_PROBE_CHILD_EOF"]
    passed = (pid is not None and (not child or (child_pid is not None and child_pid != pid)) and events == expected
              and lines.count("SCARLET_ANDROID_PROGRAM_EXIT 0") == 1
              and lines.count("SCARLET_ANDROID_STOCK_OK") == 1
              and lines.count("SCARLET_ANDROID_PROGRAM_DONE") == 1)
    return {"result": "PASS" if passed else "INCOMPLETE", "pid": pid,
            "queries": ["--query-abi-list", "--get-pid"] if passed else [],
            "child_pid": child_pid, "child_specialization": "PASS" if child and passed else "not tested",
            "child_exit_status": "not observed by supervisor", "activity_launch": "not tested"}
