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

from art_inputs import Ext4, sha256

SOURCE = Path(__file__).resolve().parent


def main():
    arguments = sys.argv[1:]
    observation=20
    if "--observe-seconds" in arguments:
        index=arguments.index("--observe-seconds");observation=int(arguments[index+1]);del arguments[index:index+2]
        if not 20<=observation<=480:raise ValueError("observation must be 20..480 seconds")
    dump_java="--dump-java-stacks" in arguments
    if dump_java:arguments.remove("--dump-java-stacks")
    if dump_java:arguments.extend(["--program-env","SCARLET_INIT_JAVA_DUMP=1"])
    verbose_logs="--verbose-logs" in arguments
    if verbose_logs:arguments.remove("--verbose-logs")
    compositor=None
    capture_frame="--capture-frame" in arguments
    if capture_frame:arguments.remove("--capture-frame")
    manage_apps="--manage-app-policy" in arguments
    if manage_apps:arguments.remove("--manage-app-policy")
    if manage_apps and "--boot-profile" not in arguments:raise ValueError("app policy management requires a boot profile")
    if "--sws-native" in arguments:
        index=arguments.index("--sws-native");compositor=Path(arguments[index+1]).resolve();del arguments[index:index+2]
    if capture_frame and not compositor:raise ValueError("frame capture requires --sws-native")
    launcher_app=None
    if "--launcher-app" in arguments:
        index=arguments.index("--launcher-app");launcher_app=Path(arguments[index+1]).resolve();del arguments[index:index+2]
    stock_product=None
    if "--stock-product" in arguments:
        index=arguments.index("--stock-product");stock_product=Path(arguments[index+1]).resolve();del arguments[index:index+2]
        if not stock_product.is_file():raise ValueError("stock product image is missing")
    if "--init-probe" not in arguments or "--boot-profile" not in arguments:
        raise ValueError("run-init requires --init-probe BUILD and --boot-profile INPUTS")
    index = arguments.index("--init-probe")
    build = Path(arguments[index + 1]).resolve()
    del arguments[index:index + 2]
    record = json.loads((build / "build.json").read_text())
    if record.get("kind") != "bounded-stock-init-probe-v1" or record.get("result") != "BUILT":
        raise ValueError("incomplete init probe build")
    for name in ("init-scope-probe.c", "init-log-probe.c", "init-signal-helper.c",
                 "android-runtime-shim.S"):
        if record["sources_sha256"].get(name) != sha256(SOURCE / name):
            raise ValueError("stale init probe source")
    for name in ("init-scope-probe", "libprobe-init-kmsg.so", "init-signal-helper",
                 "libscarlet-android-shim.so"):
        if record["files"].get(name) != sha256(build / name):
            raise ValueError("changed init probe binary")
    if any(flag in arguments for flag in ["--program", "--init-bootstrap", "--accel"]):
        raise ValueError("run-init selects its bounded program and HVF accelerator")
    launcher_app_record=None
    launcher_app_apk=None
    launcher_app_wallpaper=None
    if launcher_app:
        launcher_app_record=json.loads((launcher_app/"build.json").read_text())
        launcher_app_apk=launcher_app/"scarlet-compat.apk"
        launcher_app_wallpaper=launcher_app/"wallpaper.png"
        if (launcher_app_record.get("package")!="dev.scarlet.compat" or
                launcher_app_record.get("sha256")!=sha256(launcher_app_apk) or
                launcher_app_record.get("wallpaper_sha256")!=sha256(launcher_app_wallpaper)):
            raise ValueError("changed or incomplete launcher app build")
    stock_product_apps=[] if stock_product is None else [
        ("app", "Camera2", "Camera2.apk", "Camera2.apk"),
    ]
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
        if launcher_app:
            destination=root/"system/app/ScarletCompat/ScarletCompat.apk"
            destination.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(launcher_app_apk,destination)
            if sha256(destination)!=launcher_app_record["sha256"]:
                raise ValueError("launcher app changed while staging")
            wallpaper=root/"system/etc/scarlet/wallpaper.png"
            wallpaper.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(launcher_app_wallpaper,wallpaper)
            if sha256(wallpaper)!=launcher_app_record["wallpaper_sha256"]:
                raise ValueError("launcher wallpaper changed while staging")
            # Seed user-0's static wallpaper as the manager-owned display
            # policy. WallpaperManagerService otherwise starts from its
            # wallpaper-free ImageWallpaper fallback on every fresh data image.
            user_wallpaper=root/"data/system/users/0/wallpaper"
            user_wallpaper.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(launcher_app_wallpaper,user_wallpaper)
            user_wallpaper.chmod(0o644)
            wallpaper_info=user_wallpaper.parent/"wallpaper_info.xml"
            wallpaper_info.write_text(
                "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n"
                '<wp width="640" height="480" name="Scarlet" backup="true" '
                'cropLeft="0" cropTop="0" cropRight="640" cropBottom="480" />\n')
            wallpaper_info.chmod(0o644)
        if stock_product:
            product=Ext4(stock_product)
            permissions=root/"product/etc/permissions"
            permissions.mkdir(parents=True,exist_ok=True)
            for name,_mode,_size in product.entries("/etc/permissions"):
                if name.endswith(".xml"):
                    product.dump(f"/etc/permissions/{name}",permissions/name)
            for directory,name,source_name,filename in stock_product_apps:
                source=f"/{directory}/{name}/{source_name}"
                # Treat the selected product APKs as regular system applications
                # in this minimal image. The product partition in the pinned
                # emulator image has additional mount and policy assumptions that
                # are not present in the direct-execution view.
                destination=root/"system/app"/name/filename
                product.dump(source,destination)
        # LD_PRELOAD is inherited across exec, but Bionic resolves it again in
        # the executable's isolated linker namespace. Mirror the diagnostic
        # libraries into each APEX's own lib directory as well as the system and
        # vendor namespaces; /system/lib64 is not a search path for APEX hosts.
        library_directories={root/"system/lib64",root/"vendor/lib64",*root.glob("*/apex/*/lib64")}
        for library_directory in sorted(library_directories):
            destination=library_directory/"libprobe-init-kmsg.so"
            destination.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(build/"libprobe-init-kmsg.so",destination)
            if sha256(destination)!=record["files"]["libprobe-init-kmsg.so"]:raise ValueError("log probe changed while staging")
            shim=library_directory/"libscarlet-android-shim.so"
            shutil.copy2(build/"libscarlet-android-shim.so",shim)
            if sha256(shim)!=record["files"]["libscarlet-android-shim.so"]:raise ValueError("runtime shim changed while staging")
        helper=root/"system/bin/scarlet-signal-helper"
        shutil.copy2(build/"init-signal-helper",helper)
        if sha256(helper)!=record["files"]["init-signal-helper"]:raise ValueError("signal helper changed while staging")
        shutil.copy2(build / "build.json", output / "init-probe-build.json")
        (output / "init-diagnostic.json").write_text(json.dumps({
            "kind": "bounded-stock-init-profile-v1", "probe_build_sha256": sha256(build / "build.json"),
            "runner_sha256": sha256(SOURCE / "run-init.py"), "acceleration": "hvf",
            "observation": f"{observation} seconds or init exit, then close ProcessScope and verify zero live members",
            "logging": "Diagnostic preload sends init/daemon messages to inherited console and Zygote messages through short-lived Unix datagrams; supervisor drains logdw wire records; runtime results remain unchanged",
            "service_readiness": "not asserted",
            "service_start": "stock init zygote-start and nonencrypted actions",
            "apex_activation": "stock apexd must perform activation",
            "app_policy_manager": manage_apps,
            "launcher_app": None if not launcher_app else {
                "package": launcher_app_record["package"],
                "apk_sha256": launcher_app_record["sha256"],
                "wallpaper_sha256": launcher_app_record["wallpaper_sha256"],
                "path": "/system/app/ScarletCompat/ScarletCompat.apk",
                "wallpaper_path": "/system/etc/scarlet/wallpaper.png",
                "user_wallpaper_path": "/data/system/users/0/wallpaper",
            },
            "stock_product": None if not stock_product else {
                "path": str(stock_product),
                "sha256": sha256(stock_product),
                "packages": [f"{directory}/{name}/{source_name}" for directory,name,source_name,_filename in stock_product_apps],
            },
        }, indent=2) + "\n")
        return metadata

    arguments.extend(["--program-env",f"SCARLET_INIT_OBSERVE_SECONDS={observation}"])
    arguments.extend(["--program-env",f"ANDROID_LOG_TAGS={'*:v' if verbose_logs else '*:e'}"])
    arguments.extend(["--program-env",f"SCARLET_ANDROID_LOG_MIN_PRIORITY={'0' if verbose_logs else '4'}"])
    if not verbose_logs:
        arguments.extend(["--program-env","SCARLET_INIT_QUIET=1"])
    runner.stage_boot_inputs = stage
    original_run=runner.smoke.run_guest
    previous=os.environ.get("SCARLET_ANDROID_SWS_NATIVE")
    previous_policy=os.environ.get("SCARLET_ANDROID_APP_POLICY")
    if manage_apps:os.environ["SCARLET_ANDROID_APP_POLICY"]="1"
    if compositor:
        os.environ["SCARLET_ANDROID_SWS_NATIVE"]="1"
        def run(command,directory,timeout):
            command=list(command)+[
                "-device","virtio-gpu-device,bus=virtio-mmio-bus.3",
                "-device","virtio-mouse-device,bus=virtio-mmio-bus.4",
                "-device","virtio-keyboard-device,bus=virtio-mmio-bus.5",
            ]
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
                        if b"SCARLET_ANDROID_FRAMEBUFFER_READY" not in data:continue
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
