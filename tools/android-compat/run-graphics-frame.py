#!/usr/bin/env python3
"""Present a real Android framebuffer-HAL test frame through a private SWS."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import struct
import sys
import tempfile
import threading
import time
import zlib

from art_inputs import sha256

SOURCE=Path(__file__).resolve().parent
ROOT=SOURCE.parent.parent


def capture_pixels(ppm, png):
    """Read QEMU's bounded P6 screenshot and encode RGB PNG using stdlib."""
    raw=ppm.read_bytes()
    header=re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\r?\n",raw)
    if not header:raise ValueError("unexpected QEMU PPM header")
    width,height=map(int,header.groups())
    if not 0<width<=4096 or not 0<height<=4096:raise ValueError("screenshot dimensions out of bounds")
    pixels=raw[header.end():]
    if len(pixels)!=width*height*3:raise ValueError("invalid screenshot payload length")
    def chunk(kind,data):
        return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data))
    rows=b"".join(b"\0"+pixels[start:start+width*3] for start in range(0,len(pixels),width*3))
    png.write_bytes(b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",width,height,8,2,0,0,0))+
                    chunk(b"IDAT",zlib.compress(rows))+chunk(b"IEND",b""))
    expected=[bytes(color) for color in [(240,32,40),(24,220,40),(24,32,230)]]
    counts=[sum(pixels[i:i+3]==color for i in range(0,len(pixels),3)) for color in expected]
    return {"size":[width,height],"expected_rgb_counts":counts,"frame_sha256":sha256(png)}


def main():
    arguments=sys.argv[1:]
    selected={}
    for name in ["sws-native","sws-client","graphics-hal"]:
        flag="--"+name
        if flag not in arguments:raise ValueError("required: "+flag)
        index=arguments.index(flag);selected[name]=Path(arguments[index+1]).resolve();del arguments[index:index+2]
    if any(flag in arguments for flag in ["--program","--accel","--program-env","--program-argument"]):
        raise ValueError("graphics frame runner selects its program, environment and HVF accelerator")
    output=Path(arguments[arguments.index("--output")+1]).resolve()
    builds={}
    for name,kind in [("sws-client","android-sws-client-v1"),("graphics-hal","android-cpu-graphics-hal-v1")]:
        record=json.loads((selected[name]/"build.json").read_text())
        if record.get("kind")!=kind:raise ValueError("unexpected graphics build kind")
        for path,digest in record["files"].items():
            if sha256(selected[name]/path)!=digest:raise ValueError("changed graphics build artifact")
        for path,digest in record["sources"].items():
            actual=ROOT/path if name=="sws-client" else SOURCE/path
            if sha256(actual)!=digest:raise ValueError("stale graphics source; rebuild")
        builds[name]=record
    sws_digest=sha256(selected["sws-native"])
    spec=importlib.util.spec_from_file_location("graphics_frame_direct",SOURCE/"run-direct.py")
    runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
    original_stage=runner.stage_boot_inputs
    def stage(inputs,runtime,stock,android):
        record=original_stage(inputs,runtime,stock,android)
        native=android.parent.parent/"bin/sws";shutil.copy2(selected["sws-native"],native)
        if sha256(native)!=sws_digest:raise ValueError("changed native compositor during staging")
        for original,destination in [(selected["sws-client"]/"libsws_client_c.so",android/"vendor/lib64/libsws_client_c.so"),
                                     (selected["graphics-hal"]/"gralloc.scarlet.so",android/"vendor/lib64/hw/gralloc.scarlet.so")]:
            if destination.exists():raise ValueError("graphics overlay conflicts with a stock input")
            destination.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(original,destination)
            if sha256(original)!=sha256(destination):raise ValueError("changed graphics overlay")
        for name in builds:shutil.copy2(selected[name]/"build.json",output/(name+"-build.json"))
        (output/"graphics-inputs.json").write_text(json.dumps({"native_sws":str(selected["sws-native"]),
            "native_sws_sha256":sws_digest,"runner_sha256":sha256(SOURCE/"run-graphics-frame.py"),
            "native_supervisor_sha256":sha256(SOURCE/"init/src/native_sws.rs"),
            "scope":"private HVF guest; CPU compositor and Android framebuffer HAL test pattern; no APK or SurfaceFlinger claim"},indent=2)+"\n")
        return record
    runner.stage_boot_inputs=stage
    original_run=runner.smoke.run_guest
    sys.path.insert(0,str(ROOT/"tools"))
    from qmp import Qmp
    capture={}
    def run(command,directory,timeout):
        stopped=threading.Event()
        with tempfile.TemporaryDirectory(prefix="scarlet-android-gfx-",dir="/tmp") as temp:
            socket=str(Path(temp)/"qmp.sock")
            command=list(command)+["-device","virtio-gpu-device,bus=virtio-mmio-bus.3","-qmp",f"unix:{socket},server=on,wait=off"]
            record=json.loads((directory/"commands.json").read_text());record["qemu"]=command
            (directory/"commands.json").write_text(json.dumps(record,indent=2)+"\n")
            def screenshot():
                try:
                    while not stopped.wait(.05):
                        serial=directory/"serial.log"
                        if serial.exists():
                            data=serial.read_bytes()
                            if b"SCARLET_ANDROID_FRAMEBUFFER_PRESENTED" in data and b"[Compositor] Starting main loop" in data:break
                    if stopped.is_set():return
                    if stopped.wait(.4):return
                    with Qmp(socket,timeout=3) as qmp:qmp.execute("screendump",{"filename":str(output/"frame.ppm")})
                    capture.update(capture_pixels(output/"frame.ppm",output/"frame.png"))
                except Exception as error:capture["error"]=str(error)
            worker=threading.Thread(target=screenshot,daemon=True);worker.start()
            try:return original_run(command,directory,timeout)
            finally:stopped.set();worker.join(timeout=4)
    runner.smoke.run_guest=run
    previous=os.environ.get("SCARLET_ANDROID_SWS_NATIVE");os.environ["SCARLET_ANDROID_SWS_NATIVE"]="1"
    try:
        sys.argv=[str(SOURCE/"run-direct.py"),*arguments,"--program",str(selected["graphics-hal"]/"gralloc-frame-probe"),
                  "--program-argument=--framebuffer","--program-env","LD_LIBRARY_PATH=/vendor/lib64:/system/lib64:/apex/com.android.runtime/lib64/bionic",
                  "--init-bootstrap","--accel","hvf"]
        result=runner.main()
    finally:
        if previous is None:os.environ.pop("SCARLET_ANDROID_SWS_NATIVE",None)
        else:os.environ["SCARLET_ANDROID_SWS_NATIVE"]=previous
    serial=(output/"serial.log").read_bytes()
    cleanup=re.search(rb"SCARLET_ANDROID_SWS_STOPPED pid=(\d+) waited=(\d+)",serial)
    capture["compositor_reaped"]=bool(cleanup and cleanup[1]==cleanup[2])
    capture["result"]="PASS" if (result==0 and b"SCARLET_ANDROID_FRAMEBUFFER_OK" in serial and
        capture["compositor_reaped"] and all(n>=1000 for n in capture.get("expected_rgb_counts",[0]))) else "FAIL"
    capture["apk_execution"]="not tested"
    (output/"graphics-result.json").write_text(json.dumps(capture,indent=2)+"\n")
    print("Android framebuffer through SWS:",capture["result"])
    return 0 if capture["result"]=="PASS" else 1


if __name__=="__main__":raise SystemExit(main())
