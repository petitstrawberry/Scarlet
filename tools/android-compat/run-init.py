#!/usr/bin/env python3
"""Observe stock init under an explicit manager profile, without asserting boot readiness."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import threading

from art_inputs import sha256

SOURCE = Path(__file__).resolve().parent


def main():
    arguments = sys.argv[1:]
    observation=20
    if "--observe-seconds" in arguments:
        index=arguments.index("--observe-seconds");observation=int(arguments[index+1]);del arguments[index:index+2]
        if not 20<=observation<=300:raise ValueError("observation must be 20..300 seconds")
    compositor=None
    capture_frame="--capture-frame" in arguments
    if capture_frame:arguments.remove("--capture-frame")
    start_installer="--start-installer" in arguments
    if start_installer:arguments.remove("--start-installer")
    start_netd="--start-netd" in arguments
    if start_netd:arguments.remove("--start-netd")
    manage_apps="--manage-app-policy" in arguments
    if manage_apps:arguments.remove("--manage-app-policy")
    if manage_apps and "--boot-profile" not in arguments:raise ValueError("app policy management requires a boot profile")
    if "--sws-native" in arguments:
        index=arguments.index("--sws-native");compositor=Path(arguments[index+1]).resolve();del arguments[index:index+2]
    if capture_frame and not compositor:raise ValueError("frame capture requires --sws-native")
    start_zygote="--start-zygote" in arguments
    if start_zygote:arguments.remove("--start-zygote")
    if "--init-probe" not in arguments or "--boot-profile" not in arguments:
        raise ValueError("run-init requires --init-probe BUILD and --boot-profile INPUTS")
    index = arguments.index("--init-probe")
    build = Path(arguments[index + 1]).resolve()
    del arguments[index:index + 2]
    record = json.loads((build / "build.json").read_text())
    if record.get("kind") != "bounded-stock-init-probe-v1" or record.get("result") != "BUILT":
        raise ValueError("incomplete init probe build")
    for name in ("init-scope-probe.c", "init-log-probe.c"):
        if record["sources_sha256"].get(name) != sha256(SOURCE / name):
            raise ValueError("stale init probe source")
    for name in ("init-scope-probe", "libprobe-init-kmsg.so"):
        if record["files"].get(name) != sha256(build / name):
            raise ValueError("changed init probe binary")
    if any(flag in arguments for flag in ["--program", "--init-bootstrap", "--accel"]):
        raise ValueError("run-init selects its bounded program and HVF accelerator")
    output = Path(arguments[arguments.index("--output") + 1])
    spec = importlib.util.spec_from_file_location("direct_init_runner", SOURCE / "run-direct.py")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    original = runner.stage_boot_inputs

    def stage(inputs, runtime, stock, root):
        metadata = original(inputs, runtime, stock, root)
        if compositor:
            destination=root.parent.parent/"bin/sws";shutil.copy2(compositor,destination)
            if sha256(destination)!=sha256(compositor):raise ValueError("changed native SWS executable")
            (output/"native-sws.json").write_text(json.dumps({"path":str(compositor),"sha256":sha256(compositor),"backend":"cpu"},indent=2)+"\n")
        for partition in ["system","vendor"]:
            destination=root/partition/"lib64/libprobe-init-kmsg.so"
            destination.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(build/"libprobe-init-kmsg.so",destination)
            if sha256(destination)!=record["files"]["libprobe-init-kmsg.so"]:raise ValueError("log probe changed while staging")
        shutil.copy2(build / "build.json", output / "init-probe-build.json")
        (output / "init-diagnostic.json").write_text(json.dumps({
            "kind": "bounded-stock-init-profile-v1", "probe_build_sha256": sha256(build / "build.json"),
            "runner_sha256": sha256(SOURCE / "run-init.py"), "acceleration": "hvf",
            "observation": f"{observation} seconds or init exit, then close ProcessScope and verify zero live members",
            "logging": "Diagnostic preload sends init/daemon messages to inherited console and Zygote messages through short-lived Unix datagrams; supervisor drains logdw wire records; runtime results remain unchanged",
            "service_readiness": "not asserted",
            "service_control_request": "ctl.start=zygote after 4 seconds" if start_zygote else None, "apex_activation": "stock apexd must perform activation",
            "installer_control_request": "ctl.start=installd after 4 seconds" if start_installer else None,
            "netd_control_request": "ctl.start=netd after 4 seconds" if start_netd else None,
            "app_policy_manager": manage_apps,
        }, indent=2) + "\n")
        return metadata

    if start_zygote:arguments.extend(["--program-argument","start-zygote"])
    arguments.extend(["--program-env",f"SCARLET_INIT_OBSERVE_SECONDS={observation}"])
    if start_installer:arguments.extend(["--program-env","SCARLET_INIT_START_INSTALLER=1"])
    if start_netd:arguments.extend(["--program-env","SCARLET_INIT_START_NETD=1"])
    runner.stage_boot_inputs = stage
    original_run=runner.smoke.run_guest
    previous=os.environ.get("SCARLET_ANDROID_SWS_NATIVE")
    previous_policy=os.environ.get("SCARLET_ANDROID_APP_POLICY")
    if manage_apps:os.environ["SCARLET_ANDROID_APP_POLICY"]="1"
    if compositor:
        os.environ["SCARLET_ANDROID_SWS_NATIVE"]="1"
        def run(command,directory,timeout):
            command=list(command)+["-device","virtio-gpu-device,bus=virtio-mmio-bus.3"]
            def record_command():
                commands=json.loads((directory/"commands.json").read_text());commands["qemu"]=command
                (directory/"commands.json").write_text(json.dumps(commands,indent=2)+"\n")
            if not capture_frame:
                record_command();return original_run(command,directory,timeout)
            sys.path.insert(0,str(SOURCE.parent.parent/"tools"))
            from qmp import Qmp
            spec=importlib.util.spec_from_file_location("init_frame_capture",SOURCE/"run-graphics-frame.py")
            pixels=importlib.util.module_from_spec(spec);spec.loader.exec_module(pixels)
            with tempfile.TemporaryDirectory(prefix="scarlet-init-frame-",dir="/tmp") as temporary:
                socket=str(Path(temporary)/"qmp.sock")
                command.extend(["-qmp",f"unix:{socket},server=on,wait=off"]);record_command()
                stopped=threading.Event();capture={"scope":"observed SWS display; app readiness is not asserted","snapshots":0}
                def screenshot():
                    while not stopped.wait(2):
                        serial=directory/"serial.log"
                        if not serial.exists():continue
                        data=serial.read_bytes()
                        if b"SCARLET_INIT_NAMESPACE_STOP_REAP" in data:return
                        if b"Finished setting power mode 2 on display 0" not in data:continue
                        try:
                            with Qmp(socket,timeout=3) as qmp:qmp.execute("screendump",{"filename":str(directory/"frame.ppm")})
                            proof=pixels.capture_pixels(directory/"frame.ppm",directory/"frame.png")
                            capture.update({key:proof[key] for key in ["size","frame_sha256"]});capture["snapshots"]+=1
                        except Exception as error:capture["last_error"]=str(error)
                worker=threading.Thread(target=screenshot,daemon=True);worker.start()
                try:return original_run(command,directory,timeout)
                finally:
                    stopped.set();worker.join(timeout=4)
                    (directory/"frame-observation.json").write_text(json.dumps(capture,indent=2)+"\n")
        runner.smoke.run_guest=run
    sys.argv = [str(SOURCE / "run-direct.py"), *arguments, "--program", str(build / "init-scope-probe"), "--init-bootstrap", "--accel", "hvf"]
    try:return runner.main()
    finally:
        if manage_apps:
            if previous_policy is None:os.environ.pop("SCARLET_ANDROID_APP_POLICY",None)
            else:os.environ["SCARLET_ANDROID_APP_POLICY"]=previous_policy
        if compositor:
            if previous is None:os.environ.pop("SCARLET_ANDROID_SWS_NATIVE",None)
            else:os.environ["SCARLET_ANDROID_SWS_NATIVE"]=previous


if __name__ == "__main__":
    raise SystemExit(main())
