#!/usr/bin/env python3
"""Prepare verified factory payloads and explicit manager configuration for stock init."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import stat
import tempfile

from art_inputs import Ext4, SDK_SHA256, elf_needed, quote_debugfs, sha256, unpack_apex
from boot_profile import MANIFEST, POLICY, guest_alias, relative_path, shield_policy, validate_profile, verify_boot_inputs
from stock_art import apex_identity, extract_partitions, verify_stock_inputs

SOURCE = Path(__file__).resolve().parent


def entries(image, source):
    info = image.command("stat " + quote_debugfs(source))
    if "Type: directory" not in info:
        raise ValueError(f"not an ext4 directory: {source}")
    result = []
    for row in image.command("ls -p " + quote_debugfs(source)).splitlines():
        if not row.strip():
            continue
        match = re.fullmatch(r"/(\d+)/([0-7]+)/\d+/\d+/([^/]*)/([0-9]*)/", row)
        if not match:
            raise ValueError(f"malformed ext4 listing: {row}")
        if int(match[1]) == 0 or match[3] in (".", ".."):
            continue
        relative_path(match[3])
        if "/" in match[3]:
            raise ValueError("invalid ext4 entry name")
        result.append((match[3], int(match[2], 8), int(match[4] or 0)))
    return result


def tree(image, source, destination, output, aliases):
    relative_path(destination)
    (output / destination).mkdir(parents=True, exist_ok=True)
    for name, mode, size in entries(image, source):
        original, relative = source.rstrip("/") + "/" + name, destination + "/" + name
        relative_path(relative)
        if stat.S_ISDIR(mode):
            tree(image, original, relative, output, aliases)
        elif stat.S_ISREG(mode):
            image.dump(original, output / relative, size)
            (output / relative).chmod(mode & 0o777)
        elif stat.S_ISLNK(mode):
            info = image.command("stat " + quote_debugfs(original))
            match = re.search(r'^Fast link dest: "([^"\n]+)"$', info, re.MULTILINE)
            if not match:
                raise ValueError(f"unsupported image link storage: {original}")
            # Retain guest links as manifest data, never as host filesystem links.
            aliases[relative] = guest_alias(relative, match[1])
        else:
            raise ValueError(f"unsupported payload node: {original}")


def prepare(archive, runtime, stock, profile_file, output):
    base = verify_stock_inputs(stock, runtime)
    profile = validate_profile(json.loads(profile_file.read_text()))
    output.mkdir(parents=True, exist_ok=False)
    manifest = output / MANIFEST
    manifest.write_text('{"result":"PREPARING"}\n')
    aliases, payloads, needed = {}, [], {}
    contexts = [name for name in base["files"] if name.endswith("_property_contexts")]
    metadata = {"kind": "scarlet-android-boot-inputs-v1", "result": "PREPARING", "sdk_sha256": SDK_SHA256,
                "runtime_manifest_sha256": sha256(runtime / "art-runtime.json"),
                "stock_manifest_sha256": sha256(stock / "stock-inputs.json"),
                "profile": profile, "profile_source_sha256": sha256(profile_file),
                "property_context_files": contexts, "replacements": {}}
    with tempfile.TemporaryDirectory(prefix="boot-inputs-", dir=output) as directory:
        temporary = Path(directory)
        metadata["partitions"] = extract_partitions(archive, temporary)
        system = Ext4(temporary / "system.ext4")
        selected = {record["name"]: record for record in base["factory_apexes"]}
        for name in profile["apex_modules"]:
            record = selected[name]
            package, payload = temporary / Path(record["source"]).name, temporary / "payload.ext4"
            system.dump(record["source"], package)
            if sha256(package) != record["package_sha256"]:
                raise ValueError("factory package differs from verified stock input")
            unpack_apex(package, payload)
            if sha256(payload) != record["payload_sha256"]:
                raise ValueError("factory APEX payload changed")
            apex = Ext4(payload)
            tree(apex, "/", "system/apex/" + name, output, aliases)
            identity = apex_identity((output / "system/apex" / name / "apex_manifest.pb").read_bytes())
            if identity != (name, record["version"], record["version_name"]):
                raise ValueError("factory manifest identity mismatch")
            payloads.append(record)
            package.unlink()
            payload.unlink()
            embedded = payload.with_suffix(".apex")
            if embedded.exists():
                embedded.unlink()
        tree(system, "/system/lib64/bootstrap", "system/lib64/bootstrap", output, aliases)
        tree(system, "/system/etc/init/hw", "system/etc/init/hw", output, aliases)
        for path in profile["init_imports"]:
            if path.startswith("/system/etc/init/hw/"):
                continue
            system.dump(path, output / path[1:])
        for path in ["/system/etc/ueventd.rc", "/system/etc/selinux/plat_file_contexts"]:
            system.dump(path, output / path[1:])
        system.dump("/system/etc/task_profiles.json",output / "etc/task_profiles.json")
        queue = []
        for name in ["init", "ueventd", "apexd", "servicemanager", "boringssl_self_test64", "bootstrap/linker64", "bootstrap/linkerconfig"]:
            path = output / "system/bin" / name
            if name == "ueventd":
                info = system.command('stat "/system/bin/ueventd"')
                if 'Fast link dest: "init"' not in info:
                    raise ValueError("unexpected ueventd alias")
                aliases["system/bin/ueventd"] = "init"
                continue
            system.dump("/system/bin/" + name, path)
            queue.append(path)
        available = set(base["files"]) | set(base["symlinks"]) | set(
            json.loads((runtime / "art-runtime.json").read_text())["files"])
        seen = set()
        while queue:
            binary = queue.pop()
            key = str(binary.relative_to(output))
            if key in seen:
                continue
            seen.add(key)
            needed[key] = elf_needed(binary)
            for lib in needed[key]:
                relative = "system/lib64/" + lib
                if relative in available:
                    continue
                target = output / relative
                if not target.exists():
                    system.dump("/" + relative, target)
                queue.append(target)
    configure(output, stock, profile, contexts, metadata)
    metadata.update(result="PREPARED", activation="not executed", factory_apexes=payloads, needed=needed,
                    symlinks=aliases,
                    files={str(path.relative_to(output)): sha256(path) for path in sorted(output.rglob("*"))
                           if path.is_file() and path != manifest},
                    directories=[str(path.relative_to(output)) for path in sorted(output.rglob("*")) if path.is_dir()])
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    verify_boot_inputs(output, runtime, stock)
    print(f"Prepared {len(payloads)} flattened factory APEXes; stock init boot has not been tested")


def configure(output, stock, profile, contexts, metadata):
    from host_services import MANAGED_RC,HW_MANAGED_RC,GRAPHICS_RC,INSTALLER_MANAGED_RC,ODSIGN_MANAGED_RC,HEALTH_MANAGED_RC,VOLD_MANAGED_RC,LMKD_MANAGED_RC,NETD_MANAGED_RC,AUDIOSERVER_MANAGED_RC,AUDIO_HAL_MANAGED_RC,KEYSTORE_MANAGED_RC,KEYMINT_MANAGED_RC,imports
    for name,record in list(metadata.get("host_service_inputs",{}).items()):
        if name not in profile.get("host_services",[]):
            relative={"system_suspend":MANAGED_RC,"hwservicemanager":HW_MANAGED_RC,"graphics_cpu":GRAPHICS_RC,"installd":INSTALLER_MANAGED_RC,"odsign":ODSIGN_MANAGED_RC, "health":HEALTH_MANAGED_RC,"vold":VOLD_MANAGED_RC,"lmkd":LMKD_MANAGED_RC,"netd":NETD_MANAGED_RC,"audioserver":AUDIOSERVER_MANAGED_RC,"audio_hal":AUDIO_HAL_MANAGED_RC,"keystore2":KEYSTORE_MANAGED_RC,"keymint":KEYMINT_MANAGED_RC}[name]
            path=output/relative
            expected=record["files"][relative] if name=="graphics_cpu" else record["managed_rc_sha256"]
            if sha256(path)!=expected:raise ValueError("changed manager host service output")
            path.unlink();metadata["host_service_inputs"].pop(name)
    previous_vintf = metadata.get("vintf_inputs")
    if previous_vintf and previous_vintf["selection"] != profile.get("vintf"):
        for name, digest in previous_vintf["generated_sha256"].items():
            path = output / name
            if not path.is_file() or sha256(path) != digest:
                raise ValueError("changed manager VINTF output")
            path.unlink()
        metadata.pop("vintf_inputs")
    (output / "scarlet/boot").mkdir(parents=True, exist_ok=True)
    (output / POLICY).write_text(shield_policy(profile, [(stock / name).read_text() for name in contexts]))
    # Android 13's post-fs-data action resets this property to false, after
    # build.prop is loaded. Select the native memfd path in that action too.
    # Retain the verified original so reconfiguration can reverse the choice.
    memory_source = "system/etc/init/hw/init.rc"
    memory_original = "scarlet/boot/init-hw.rc.original"
    previous_memory = metadata.get("shared_memory_init")
    memory_init = output / memory_source
    if previous_memory:
        original = (output / memory_original).read_bytes()
        if sha256(output / memory_original) != previous_memory["source_sha256"]:
            raise ValueError("changed stock shared-memory init source")
        memory_init.write_bytes(original)
    if profile["boot_properties"].get("sys.use_memfd") == "true":
        original = memory_init.read_text()
        configured = select_memfd_in_init(original)
        (output / memory_original).write_text(original)
        memory_init.write_text(configured)
        metadata["shared_memory_init"] = {"source": memory_source,
            "source_sha256": sha256(output / memory_original),
            "configured_sha256": sha256(memory_init), "mechanism": "memfd-future-write"}
    else:
        metadata.pop("shared_memory_init", None)
        (output / memory_original).unlink(missing_ok=True)
    # Stock init loads product properties last. Keep the original file separately
    # and record the deliberate machine-profile replacement and exact old hash.
    product = "product/etc/build.prop"
    original = (stock / product).read_text()
    (output / "scarlet/boot/product-build.prop.original").write_text(original)
    (output / product).parent.mkdir(parents=True, exist_ok=True)
    (output / product).write_text(original + "\n# Scarlet manager boot profile\n" + "".join(
        f"{name}={value}\n" for name, value in sorted(profile["boot_properties"].items())))
    metadata["replacements"][product] = json.loads((stock / "stock-inputs.json").read_text())["files"][product]
    (output / "proc").mkdir(exist_ok=True)
    (output / "proc/bootconfig").write_text("".join(
        f'{name} = "{value}"\n' for name, value in sorted(profile["bootconfig"].items())))
    manager_init = "# Explicit Scarlet device boot configuration\n" + "".join(
        "import " + path + "\n" for path in profile["init_imports"] + imports(profile))
    if "com.android.tethering" in profile["apex_modules"]:
        # NetworkStats opens its APEX data directory before the stock traffic
        # controller finishes initializing. Seed the persistent directory the
        # manager owns so a transient native-init failure cannot kill the
        # system server during first boot.
        manager_init += (
            "\non post-fs-data\n"
            "    mkdir /data/misc/apexdata/com.android.tethering 0771 root system\n"
            "    mkdir /data/misc/apexdata/com.android.tethering/netstats 0771 root system\n"
        )
    (output / "scarlet/boot/init.rc").write_text(manager_init)
    if metadata.get("user_unlock_inputs"):
        from user_unlock_inputs import apply_service
        apply_service(output)
    # /data_mirror is a ramdisk directory on a real device; the generic
    # Android init configuration mounts a private tmpfs over it before making
    # the per-user mirror bind mounts used for app data.
    # The outer Android manager owns volume policy: it provides the initial
    # user-0 data roots that stock vold would otherwise create after key setup.
    for name in ["storage", "apex", "linkerconfig", "data", "data_mirror", "data/user_de/0", "data/user/0", "sys/devices", "sys/class/power_supply", "sys/fs/selinux", "system_ext/apex", "product/apex", "vendor/apex"]:
        (output / name).mkdir(parents=True, exist_ok=True)
    if "lmkd" in profile.get("host_services", []):
        # The pinned lmkd accepts the kernel-lowmemory-killer interface when it
        # cannot observe PSI or vmpressure. Scarlet currently has neither monitor;
        # expose the writable target file so the service remains available to AMS.
        parameter = output / "sys/module/lowmemorykiller/parameters/minfree"
        parameter.parent.mkdir(parents=True, exist_ok=True)
        parameter.write_text("18432,23040,27648,32256,55296,80640\n")
        parameter.chmod(0o666)
        event = output / "proc/lowmemorykiller"
        event.parent.mkdir(parents=True, exist_ok=True)
        event.write_text("")
    if "resource_groups" in profile:
        (output / "etc").mkdir(exist_ok=True)
        cpusets = []
        if "cpuset_root" in profile["resource_groups"]:
            cpusets.append({"Controller": "cpuset", "Path": profile["resource_groups"]["cpuset_root"], "Mode": "0755", "UID": "system", "GID": "system"})
        (output / "etc/cgroups.json").write_text(json.dumps({"Cgroups": cpusets, "Cgroups2": {
            "Path": profile["resource_groups"]["cgroup2_root"], "Mode": "0755", "UID": "root", "GID": "root", "Controllers": [],
        }}, indent=2) + "\n")
        (output / "sys/fs/cgroup").mkdir(parents=True, exist_ok=True)


def select_memfd_in_init(text):
    command = "    setprop sys.use_memfd false\n"
    if text.count(command) != 1:
        raise ValueError("unexpected stock shared-memory init action")
    return text.replace(command, "    setprop sys.use_memfd true\n")


def reconfigure(inputs, runtime, stock, profile_file, output, vintf_image=None, service_image=None, firewall_compat=None, audio_start=None, audio_binder_compat=None):
    metadata = verify_boot_inputs(inputs, runtime, stock)
    profile = validate_profile(json.loads(profile_file.read_text()))
    if any(profile[key] != metadata["profile"][key] for key in ("apex_modules", "init_imports")):
        raise ValueError("changed payload or imports require preparation from the pinned image")
    added_services=set(profile.get("host_services",[]))-set(metadata["profile"].get("host_services",[]))
    # Reusing verified payloads must not implicitly require a new SDK archive.
    # Provider refreshes are explicitly requested by supplying that archive.
    refreshed_services={name for name in profile.get("host_services",[])
                        if name=="netd" and service_image is not None and not added_services}
    if service_image is not None and not added_services:
        refreshed_services |= {name for name in profile.get("host_services",[]) if name in ("audioserver","audio_hal")}
    providers_to_add=added_services|refreshed_services
    if providers_to_add and (not providers_to_add.issubset({"vold","lmkd","netd","audioserver","audio_hal","keystore2","keymint"}) or not service_image):
        raise ValueError("adding a stock provider requires --service-image; supported additions: vold, lmkd, netd, audioserver, audio_hal, keystore2, keymint")
    if "netd" in providers_to_add and not firewall_compat:
        raise ValueError("netd provider requires --netd-firewall-compat")
    if providers_to_add.intersection({"audioserver","audio_hal"}) and not audio_start:
        raise ValueError("audio providers require --audio-start-build")
    if "audio_hal" in providers_to_add and not audio_binder_compat:
        raise ValueError("audio HAL requires --audio-binder-compat")
    refreshed_vintf = None
    if profile.get("vintf") != metadata["profile"].get("vintf"):
        if not vintf_image:
            raise ValueError("changed VINTF selection requires --vintf-image")
        previous = metadata.get("vintf_inputs", {})
        previous_device=previous.get("selection", {}).get("device",[])
        selected_device=profile.get("vintf", {}).get("device",[])
        from vintf_inputs import AUDIO_CONTRACT, AUDIO_EFFECT_CONTRACT, AUDIO_EFFECT_LEGACY_CONTRACT, KEYMINT_DEVICE
        from host_services import AUDIO_HAL, KEYMINT, KEYMINT_BINARY, KEYMINT_STOCK_RC, KEYMINT_MANIFESTS, build_keymint
        audio_contracts={AUDIO_CONTRACT,AUDIO_EFFECT_CONTRACT,AUDIO_EFFECT_LEGACY_CONTRACT}
        audio_additions = set(selected_device) - set(previous_device)
        audio_removals = set(previous_device) - set(selected_device)
        audio_inventory_change = (
            (audio_additions <= audio_contracts and not audio_removals) or
            (audio_additions == {AUDIO_EFFECT_CONTRACT} and
             audio_removals == {AUDIO_EFFECT_LEGACY_CONTRACT}))
        keymint_contracts=set(KEYMINT_DEVICE)
        keymint_inventory_change = (set(selected_device)-set(previous_device)==keymint_contracts and
                                    not set(previous_device)-set(selected_device) and KEYMINT in providers_to_add)
        if (selected_device != previous_device and
                not (audio_inventory_change and AUDIO_HAL in providers_to_add) and
                not keymint_inventory_change):
            raise ValueError("changed device inventory requires matching provider preparation")
        from vintf_inputs import build_manifests
        from vintf_inputs import AUDIO_CONTRACT
        from host_services import AUDIO_HAL,AUDIO_HAL_BINARY
        with tempfile.TemporaryDirectory(prefix="scarlet-vintf-") as directory:
            temporary = Path(directory)
            extract_partitions(vintf_image, temporary)
            system = Ext4(temporary / "system.ext4")
            paths = ["/system/etc/vintf/manifest.xml"]
            paths += ["/system/etc/vintf/manifest/" + name for name, mode, _ in
                      entries(system, "/system/etc/vintf/manifest")
                      if stat.S_ISREG(mode) and name.endswith(".xml")]
            if len(paths) > 64:
                raise ValueError("VINTF source count exceeds bound")
            sources = {}
            for name in paths:
                destination = temporary / "sources" / name[1:]
                system.dump(name, destination)
                sources[name] = destination.read_bytes()
            providers=dict(previous.get("device_providers",{}))
            providers.pop(AUDIO_EFFECT_LEGACY_CONTRACT, None)
            if KEYMINT in providers_to_add:
                vendor=Ext4(temporary/"vendor.ext4")
                rc=temporary/"keymint.rc";binary=temporary/"keymint.bin"
                vendor.dump("/"+KEYMINT_STOCK_RC.removeprefix("vendor/"),rc)
                vendor.dump("/"+KEYMINT_BINARY.removeprefix("vendor/"),binary)
                manifests={}
                for contract,path in KEYMINT_MANIFESTS.items():
                    target=temporary/("vintf-"+path.rsplit("/",1)[-1])
                    vendor.dump("/"+path.removeprefix("vendor/"),target)
                    manifests[contract]=target.read_bytes()
                _,_,keymint_providers=build_keymint(rc.read_bytes(),binary.read_bytes(),manifests)
                providers.update(keymint_providers)
            if AUDIO_HAL in providers_to_add:
                vendor=Ext4(temporary/"vendor.ext4");binary=temporary/AUDIO_HAL_BINARY
                vendor.dump("/"+AUDIO_HAL_BINARY.removeprefix("vendor/"),binary)
                declaration=temporary/"audio-vintf.xml";declaration_path="/vendor/etc/vintf/manifest/android.hardware.audio@7.1-impl.ranchu.xml"
                vendor.dump("/etc/vintf/manifest/android.hardware.audio@7.1-impl.ranchu.xml",declaration)
                audio_provider={"transport":"hwbinder","path":AUDIO_HAL_BINARY,
                    "sha256":sha256(binary),"source":declaration_path,
                    "source_sha256":sha256(declaration)}
                providers[AUDIO_CONTRACT]=audio_provider
                providers[AUDIO_EFFECT_CONTRACT]=dict(audio_provider)
            refreshed_vintf = build_manifests(profile["vintf"], sources, providers)
    original_manifest = sha256(inputs / MANIFEST)
    shutil.copytree(inputs, output)
    metadata.update(profile=profile, profile_source_sha256=sha256(profile_file), reused_payload_manifest_sha256=original_manifest)
    configure(output,stock,profile,metadata["property_context_files"],metadata)
    for provider in sorted(providers_to_add):
        add_stock_provider(output,metadata,service_image,provider,firewall_compat,audio_start,audio_binder_compat)
    if refreshed_vintf:
        outputs, proof = refreshed_vintf
        for name, data in outputs.items():
            destination = output / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        metadata["vintf_inputs"] = proof
    metadata["files"] = {str(path.relative_to(output)):sha256(path) for path in sorted(output.rglob("*")) if path.is_file() and path.name != MANIFEST}
    metadata["directories"] = [str(path.relative_to(output)) for path in sorted(output.rglob("*")) if path.is_dir()]
    (output / MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(output,runtime,stock)
    print("Prepared new manager configuration using verified factory payloads; boot has not been tested")


def add_stock_provider(output, metadata, archive, name, firewall_compat=None, audio_start=None, audio_binder_compat=None):
    service_name=name
    from host_services import (KEYSTORE_BINARY,KEYSTORE_STOCK_RC,KEYSTORE_MANAGED_RC,build_keystore,
                               KEYMINT_BINARY,KEYMINT_STOCK_RC,KEYMINT_MANAGED_RC,KEYMINT_MANIFESTS,build_keymint,
                               VOLD_BINARY,VOLD_STOCK_RC,VOLD_MANAGED_RC,build_vold,
                               LMKD_BINARY,LMKD_STOCK_RC,LMKD_MANAGED_RC,build_lmkd,
                               NETD_BINARY,NETD_STOCK_RC,NETD_MANAGED_RC,NETD_FIREWALL_BINARY,
                               NETD_FIREWALL_ALIASES,build_netd,
                               AUDIOSERVER,AUDIOSERVER_BINARY,AUDIOSERVER_STOCK_RC,AUDIOSERVER_MANAGED_RC,build_audioserver,
                               AUDIO_HAL,AUDIO_HAL_BINARY,AUDIO_HAL_STOCK_RC,AUDIO_HAL_MANAGED_RC,AUDIO_CONFIG_FILES,
                               AUDIO_HAL_MODULES,AUDIO_START_BINARY,AUDIO_BINDER_COMPAT,primary_audio_policy,build_audio_hal)
    binary_path,stock_rc,managed_rc,builder={
        "keymint":(KEYMINT_BINARY,KEYMINT_STOCK_RC,KEYMINT_MANAGED_RC,build_keymint),
        "keystore2":(KEYSTORE_BINARY,KEYSTORE_STOCK_RC,KEYSTORE_MANAGED_RC,build_keystore),
        "vold":(VOLD_BINARY,VOLD_STOCK_RC,VOLD_MANAGED_RC,build_vold),
        "lmkd":(LMKD_BINARY,LMKD_STOCK_RC,LMKD_MANAGED_RC,build_lmkd),
        "netd":(NETD_BINARY,NETD_STOCK_RC,NETD_MANAGED_RC,build_netd),
        "audioserver":(AUDIOSERVER_BINARY,AUDIOSERVER_STOCK_RC,AUDIOSERVER_MANAGED_RC,build_audioserver),
        "audio_hal":(AUDIO_HAL_BINARY,AUDIO_HAL_STOCK_RC,AUDIO_HAL_MANAGED_RC,build_audio_hal),
    }[name]
    with tempfile.TemporaryDirectory(prefix="scarlet-stock-provider-") as directory:
        temporary=Path(directory);partitions=extract_partitions(archive,temporary)
        system=Ext4(temporary/"system.ext4")
        vendor=Ext4(temporary/"vendor.ext4")
        rc=temporary/"service.rc";binary=temporary/"binary"
        def image(path):
            return vendor if path.startswith("vendor/") else system
        def image_source(path):
            return "/"+path.removeprefix("vendor/") if path.startswith("vendor/") else "/"+path
        image(stock_rc).dump(image_source(stock_rc),rc);image(binary_path).dump(image_source(binary_path),binary)
        managed,proof=(None,None) if service_name=="keymint" else builder(rc.read_bytes(),binary.read_bytes())
        available={Path(name).name for name in metadata["files"]}
        queue=[binary_path]+(AUDIO_HAL_MODULES if service_name=="audio_hal" else []);needed={};digests={}
        while queue:
            name=queue.pop()
            if name in needed:continue
            if len(needed)>=1024:raise ValueError("stock service dependency bound")
            source=temporary/"files"/name;image(name).dump(image_source(name),source)
            digest=sha256(source);destination=output/name
            if name in metadata.get("symlinks",{}) or destination.exists() and sha256(destination)!=digest:
                raise ValueError("conflicting stock service input: "+name)
            destination.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,destination)
            digests[name]=digest;needed[name]=elf_needed(source);available.add(Path(name).name)
            for dependency in needed[name]:
                relative=("vendor/lib64/" if name.startswith("vendor/") else "system/lib64/")+dependency
                if "Type: regular" in image(relative).command("stat "+quote_debugfs(image_source(relative))):
                    queue.append(relative)
                elif dependency not in available:
                    raise ValueError("missing stock service dependency: "+dependency)
        if service_name=="netd":
            firewall_record=json.loads((firewall_compat/"build.json").read_text())
            if (firewall_record.get("kind")!="scarlet-netd-firewall-compat-v1" or
                    firewall_record.get("result")!="BUILT" or
                    firewall_record.get("source_sha256")!=sha256(SOURCE/"iptables-compat.c") or
                    firewall_record.get("binary_sha256")!=sha256(firewall_compat/"iptables-restore")):
                raise ValueError("changed netd firewall compatibility build")
            source=firewall_compat/"iptables-restore";destination=output/NETD_FIREWALL_BINARY
            if NETD_FIREWALL_BINARY in metadata.get("symlinks",{}):
                raise ValueError("conflicting netd firewall implementation")
            destination.parent.mkdir(parents=True,exist_ok=True)
            if destination.is_file():destination.unlink()
            shutil.copy2(source,destination)
            digests[NETD_FIREWALL_BINARY]=sha256(source);needed[NETD_FIREWALL_BINARY]=elf_needed(source)
            for alias,target in NETD_FIREWALL_ALIASES.items():
                if alias in metadata["files"] or alias in metadata.get("symlinks",{}) and metadata["symlinks"][alias]!=target:
                    raise ValueError("conflicting netd firewall alias: "+alias)
                metadata.setdefault("symlinks",{})[alias]=target
        if service_name=="audio_hal":
            for config in AUDIO_CONFIG_FILES:
                source=temporary/"files"/config;vendor.dump("/etc/"+config.removeprefix("vendor/etc/"),source)
                destination=output/config;data=source.read_bytes()
                if config=="vendor/etc/audio_policy_configuration.xml":data=primary_audio_policy(data)
                digest=hashlib.sha256(data).hexdigest()
                if config in metadata.get("symlinks",{}):
                    raise ValueError("conflicting audio configuration: "+config)
                destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(data)
                digests[config]=digest;needed[config]=[]
        if service_name in ("audioserver","audio_hal"):
            record=json.loads((audio_start/"build.json").read_text())
            if (record.get("kind")!="scarlet-audio-late-start-v1" or record.get("result")!="BUILT" or
                    record.get("source_sha256")!=sha256(SOURCE/"audio-late-start.c") or
                    record.get("binary_sha256")!=sha256(audio_start/"scarlet-audio-late-start")):
                raise ValueError("changed audio startup helper")
            source=audio_start/"scarlet-audio-late-start";destination=output/AUDIO_START_BINARY
            destination.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,destination)
            digests[AUDIO_START_BINARY]=sha256(source);needed[AUDIO_START_BINARY]=elf_needed(source)
        if service_name=="audio_hal":
            record=json.loads((audio_binder_compat/"build.json").read_text())
            if (record.get("kind")!="scarlet-audio-binder-compat-v1" or record.get("result")!="BUILT" or
                    record.get("source_sha256")!=sha256(SOURCE/"audio-binder-compat.cpp") or
                    record.get("binary_sha256")!=sha256(audio_binder_compat/"libscarlet-audiobinder.so")):
                raise ValueError("changed audio binder compatibility build")
            source=audio_binder_compat/"libscarlet-audiobinder.so";destination=output/AUDIO_BINDER_COMPAT
            destination.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,destination)
            digests[AUDIO_BINDER_COMPAT]=sha256(source);needed[AUDIO_BINDER_COMPAT]=elf_needed(source)
        if service_name=="keymint":
            manifests={}
            for contract,path in KEYMINT_MANIFESTS.items():
                source=temporary/"keymint-manifests"/contract.rsplit(".",1)[-1]
                source.parent.mkdir(parents=True,exist_ok=True)
                vendor.dump("/"+path.removeprefix("vendor/"),source)
                manifests[contract]=source.read_bytes()
            managed,proof,_=build_keymint(rc.read_bytes(),binary.read_bytes(),manifests)
        path=output/managed_rc;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(managed)
        proof.update(native_needed=needed,files=digests,partitions=partitions)
        metadata.setdefault("host_service_inputs",{})[service_name]=proof


def merge_framework(inputs,runtime,stock,framework):
    from framework_inputs import verify_framework_inputs
    metadata=verify_boot_inputs(inputs,runtime,stock)
    verified=verify_framework_inputs(framework,runtime,stock)
    for name,digest in verified["files"].items():
        destination="system/"+name if name.startswith("apex/") else name
        path=inputs/destination
        if destination in metadata["symlinks"]:
            raise ValueError("framework file conflicts with factory alias: "+destination)
        if path.exists() and sha256(path)!=digest:
            raise ValueError("framework file conflicts with verified boot payload: "+destination)
        if name.startswith("apex/") and not path.exists():
            raise ValueError("framework requires unselected factory APEX file: "+destination)
        if not path.exists():
            path.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(framework/name,path)
        if sha256(path)!=digest: raise ValueError("changed framework input while copying")
    # Explicit manager environment, derived only from the verified image fragments.
    (inputs/"init.environ.rc").write_text("# Scarlet manager environment and verified Android classpath\non early-init\n    export ANDROID_ROOT /system\n    export ANDROID_DATA /data\n    export ANDROID_ART_ROOT /apex/com.android.art\n    export ANDROID_I18N_ROOT /apex/com.android.i18n\n    export ANDROID_TZDATA_ROOT /apex/com.android.tzdata\n    export BOOTCLASSPATH "+":".join(verified["boot_class_path"])+"\n")
    previous=metadata.get("platform_boot",{}).get("classpaths")
    if previous:
        environment=inputs/"init.environ.rc"
        text=environment.read_text()
        for name,kind in [("SYSTEMSERVERCLASSPATH",2),("STANDALONE_SYSTEMSERVER_JARS",4)]:
            paths=previous.get(str(kind),previous.get(kind,[]))
            if not isinstance(paths,list) or len(paths)>1024 or any(not isinstance(path,str) or not re.fullmatch(r"/(system/framework|apex/[A-Za-z0-9_.]+/javalib)/[A-Za-z0-9_.-]+[.]jar",path) for path in paths):raise ValueError("invalid retained server classpath")
            text+="    export "+name+" "+":".join(paths)+"\n"
        environment.write_text(text)
    metadata["framework_manifest_sha256"]=sha256(framework/"framework-inputs.json")
    metadata["files"]={str(path.relative_to(inputs)):sha256(path) for path in sorted(inputs.rglob("*")) if path.is_file() and path.name!=MANIFEST}
    metadata["directories"]=[str(path.relative_to(inputs)) for path in sorted(inputs.rglob("*")) if path.is_dir()]
    (inputs/MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(inputs,runtime,stock)
    print("Merged verified framework and stock app_process64; app launch has not been tested")


def merge_platform(inputs,runtime,stock,archive,graphics=None):
    from framework_inputs import protobuf_fields
    metadata=verify_boot_inputs(inputs,runtime,stock)
    copied={}
    with tempfile.TemporaryDirectory(prefix="platform-boot-",dir=inputs.parent) as directory:
        temporary=Path(directory)
        partitions=extract_partitions(archive,temporary)
        system=Ext4(temporary/"system.ext4")
        vendor=Ext4(temporary/"vendor.ext4")
        stock_environment=temporary/"init.environ.rc";system.dump("/init.environ.rc",stock_environment)
        environment_exports=parse_stock_environment(stock_environment.read_text())
        metadata["stock_environment"]={"source_sha256":sha256(stock_environment),"exports":environment_exports}
        if metadata["profile"].get("package_set"):
            from desktop_inputs import prepare as prepare_packages
            package_files,proof=prepare_packages(metadata["profile"],
                {"system":system,"vendor":vendor,"system_ext":Ext4(temporary/"system_ext.ext4"),"product":Ext4(temporary/"product.ext4")},temporary,entries)
            copied.update(package_files);metadata["desktop_inputs"]=proof
        def image_path(name):
            return (vendor,"/"+name.removeprefix("vendor/")) if name.startswith("vendor/") else (system,"/"+name)
        from host_services import BINARY, MANAGED_RC, STOCK_RC, build_suspend
        service_roots=[]
        if "system_suspend" in metadata["profile"].get("host_services",[]):
            rc=temporary/"suspend-stock.rc";binary=temporary/"suspend-stock.bin"
            system.dump("/"+STOCK_RC,rc);system.dump("/"+BINARY,binary)
            managed,proof=build_suspend(rc.read_bytes(),binary.read_bytes())
            destination=inputs/MANAGED_RC
            previous=metadata.get("host_service_inputs",{}).get("system_suspend",{}).get("managed_rc_sha256")
            if MANAGED_RC in metadata["symlinks"] or destination.exists() and sha256(destination)!=previous:
                raise ValueError("conflicting host service manager output")
            destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(managed)
            metadata.setdefault("host_service_inputs",{})["system_suspend"]=proof
            service_roots.append(BINARY)
        if "hwservicemanager" in metadata["profile"].get("host_services",[]):
            from host_services import HW_BINARY,HW_STOCK_RC,HW_MANAGED_RC,build_hwmanager
            rc=temporary/"hwmanager-stock.rc";binary=temporary/"hwmanager-stock.bin"
            system.dump("/"+HW_STOCK_RC,rc);system.dump("/"+HW_BINARY,binary)
            managed,proof=build_hwmanager(rc.read_bytes(),binary.read_bytes())
            destination=inputs/HW_MANAGED_RC
            previous=metadata.get("host_service_inputs",{}).get("hwservicemanager",{}).get("managed_rc_sha256")
            if HW_MANAGED_RC in metadata["symlinks"] or destination.exists() and sha256(destination)!=previous:
                raise ValueError("conflicting hardware-service manager output")
            destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(managed)
            metadata.setdefault("host_service_inputs",{})["hwservicemanager"]=proof
            service_roots.append(HW_BINARY)
            name="system/etc/selinux/plat_hwservice_contexts";target=temporary/"files"/name
            system.dump("/"+name,target);copied[name]=target
        device_providers=dict(metadata.get("vintf_inputs",{}).get("device_providers",{}))
        if "health" in metadata["profile"].get("host_services",[]):
            from host_services import HEALTH_BINARY,HEALTH_IMPL,HEALTH_STOCK_RC,HEALTH_MANAGED_RC,HEALTH_MANIFEST,build_health
            rc=temporary/"health-stock.rc";binary=temporary/"health-stock.bin";manifest=temporary/"health-stock.xml"
            vendor.dump("/"+HEALTH_STOCK_RC.removeprefix("vendor/"),rc)
            vendor.dump("/"+HEALTH_BINARY.removeprefix("vendor/"),binary)
            vendor.dump("/"+HEALTH_MANIFEST.removeprefix("vendor/"),manifest)
            managed,proof,providers=build_health(rc.read_bytes(),binary.read_bytes(),manifest.read_bytes())
            implementation=temporary/"files"/HEALTH_IMPL
            vendor.dump("/"+HEALTH_IMPL.removeprefix("vendor/"),implementation)
            proof["implementation_sha256"]=sha256(implementation)
            destination=inputs/HEALTH_MANAGED_RC
            previous=metadata.get("host_service_inputs",{}).get("health",{}).get("managed_rc_sha256")
            if HEALTH_MANAGED_RC in metadata["symlinks"] or destination.exists() and sha256(destination)!=previous:
                raise ValueError("conflicting Health service manager output")
            destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(managed)
            metadata.setdefault("host_service_inputs",{})["health"]=proof
            device_providers.update(providers);service_roots.extend([HEALTH_BINARY,HEALTH_IMPL])
        if "keymint" in metadata["profile"].get("host_services",[]):
            from host_services import KEYMINT_BINARY,KEYMINT_STOCK_RC,KEYMINT_MANAGED_RC,KEYMINT_MANIFESTS,build_keymint
            rc=temporary/"keymint-stock.rc";binary=temporary/"keymint-stock.bin"
            vendor.dump("/"+KEYMINT_STOCK_RC.removeprefix("vendor/"),rc)
            vendor.dump("/"+KEYMINT_BINARY.removeprefix("vendor/"),binary)
            manifests={}
            for contract,path in KEYMINT_MANIFESTS.items():
                source=temporary/"keymint-manifests"/contract.rsplit(".",1)[-1]
                source.parent.mkdir(parents=True,exist_ok=True)
                vendor.dump("/"+path.removeprefix("vendor/"),source)
                manifests[contract]=source.read_bytes()
            managed,proof,providers=build_keymint(rc.read_bytes(),binary.read_bytes(),manifests)
            destination=inputs/KEYMINT_MANAGED_RC
            previous=metadata.get("host_service_inputs",{}).get("keymint",{}).get("managed_rc_sha256")
            if KEYMINT_MANAGED_RC in metadata["symlinks"] or destination.exists() and sha256(destination)!=previous:
                raise ValueError("conflicting KeyMint service manager output")
            destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(managed)
            metadata.setdefault("host_service_inputs",{})["keymint"]=proof
            device_providers.update(providers);service_roots.append(KEYMINT_BINARY)
        from host_services import (KEYSTORE_BINARY,KEYSTORE_STOCK_RC,KEYSTORE_MANAGED_RC,build_keystore,
                                   INSTALLER_BINARY,INSTALLER_STOCK_RC,INSTALLER_MANAGED_RC,build_installer,
                                   ODSIGN_BINARY,ODSIGN_STOCK_RC,ODSIGN_MANAGED_RC,build_odsign,
                                   VOLD_BINARY,VOLD_STOCK_RC,VOLD_MANAGED_RC,build_vold,
                                   LMKD_BINARY,LMKD_STOCK_RC,LMKD_MANAGED_RC,build_lmkd,
                                   NETD_BINARY,NETD_STOCK_RC,NETD_MANAGED_RC,build_netd)
        for name,program,stock_rc,managed_rc,builder in [
            ("keystore2",KEYSTORE_BINARY,KEYSTORE_STOCK_RC,KEYSTORE_MANAGED_RC,build_keystore),
            ("installd",INSTALLER_BINARY,INSTALLER_STOCK_RC,INSTALLER_MANAGED_RC,build_installer),
            ("odsign",ODSIGN_BINARY,ODSIGN_STOCK_RC,ODSIGN_MANAGED_RC,build_odsign),
            ("vold",VOLD_BINARY,VOLD_STOCK_RC,VOLD_MANAGED_RC,build_vold),
            ("lmkd",LMKD_BINARY,LMKD_STOCK_RC,LMKD_MANAGED_RC,build_lmkd),
            ("netd",NETD_BINARY,NETD_STOCK_RC,NETD_MANAGED_RC,build_netd)]:
            if name not in metadata["profile"].get("host_services",[]):continue
            rc=temporary/(name+"-stock.rc");binary=temporary/(name+"-stock.bin")
            system.dump("/"+stock_rc,rc);system.dump("/"+program,binary)
            managed,proof=builder(rc.read_bytes(),binary.read_bytes())
            destination=inputs/managed_rc
            previous=metadata.get("host_service_inputs",{}).get(name,{}).get("managed_rc_sha256")
            if managed_rc in metadata["symlinks"] or destination.exists() and sha256(destination)!=previous:
                raise ValueError("conflicting stock service manager output: "+name)
            destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(managed)
            metadata.setdefault("host_service_inputs",{})[name]=proof
            service_roots.append(program)
            # vold's control protocol is driven through the stock vdc client.
            if name == "vold":
                service_roots.append("system/bin/vdc")
        if "graphics_cpu" in metadata["profile"].get("host_services",[]):
            if not graphics:raise ValueError("CPU graphics profile requires all three verified graphics builds")
            from graphics_boot import prepare as prepare_graphics
            proof,graphics_providers,roots=prepare_graphics(inputs,metadata,system,vendor,graphics,temporary)
            device_providers.update(graphics_providers)
            metadata.setdefault("host_service_inputs",{})["graphics_cpu"]=proof
            service_roots.extend(roots)
            # Extract stock runtime libraries before binding each declaration
            # to its actual built or stock provider bytes.
            for name in roots:
                image,path=image_path(name);target=temporary/"files"/name;image.dump(path,target);copied[name]=target
            for contract,provider in graphics_providers.items():
                path=provider["path"];provider["sha256"]=sha256(copied[path] if path in copied else inputs/path)
        elif graphics:raise ValueError("graphics builds require an explicit graphics host-service selection")
        if "vintf" in metadata["profile"]:
            from vintf_inputs import build_manifests
            source_paths=["/system/etc/vintf/manifest.xml"]
            source_paths += ["/system/etc/vintf/manifest/"+name for name,mode,_ in
                             entries(system,"/system/etc/vintf/manifest") if stat.S_ISREG(mode) and name.endswith(".xml")]
            if len(source_paths)>64:raise ValueError("VINTF source count exceeds bound")
            sources={}
            for name in source_paths:
                target=temporary/"vintf-sources"/name[1:]
                system.dump(name,target);sources[name]=target.read_bytes()
            outputs,proof=build_manifests(metadata["profile"]["vintf"],sources,device_providers)
            previous=metadata.get("vintf_inputs",{}).get("generated_sha256",{})
            for name,data in outputs.items():
                target=inputs/name
                if name in metadata["symlinks"] or target.exists() and sha256(target)!=previous.get(name):
                    raise ValueError("conflicting VINTF manager output")
                target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data)
            metadata["vintf_inputs"]=proof
        for name in ["system/etc/selinux/plat_service_contexts","system/etc/classpaths/systemserverclasspath.pb","system/etc/fonts.xml"]:
            target=temporary/"files"/name
            system.dump("/"+name,target)
            copied[name]=target
        font_aliases={}
        tree(system,"/system/fonts","system/fonts",temporary/"files",font_aliases)
        for target in (temporary/"files/system/fonts").rglob("*"):
            if target.is_file():copied[str(target.relative_to(temporary/"files"))]=target
        # Zygote and SystemServer load these by name, outside app_process DT_NEEDED.
        available={Path(name).name for name in metadata["files"]}
        runtime_files=json.loads((runtime/"art-runtime.json").read_text())["files"]
        available|={Path(name).name for name in runtime_files}
        native_needed={};queue=["system/lib64/libcompiler_rt.so","system/lib64/libandroid_servers.so",
                               "system/lib64/libalarm_jni.so","system/lib64/libsoundpool.so"]+service_roots;seen=set()
        while queue:
            name=queue.pop()
            if name in seen:continue
            if len(seen)>=1024:raise ValueError("platform native dependency bound")
            seen.add(name);target=temporary/"files"/name
            image,path=image_path(name)
            if not target.exists():image.dump(path,target)
            copied[name]=target;available.add(Path(name).name);native_needed[name]=elf_needed(target)
            for dependency in native_needed[name]:
                vendor_relative="vendor/lib64/"+dependency
                if name.startswith("vendor/") and "Type: regular" in vendor.command("stat "+quote_debugfs("/lib64/"+dependency)):
                    queue.append(vendor_relative);continue
                relative="system/lib64/"+dependency
                if relative in seen:continue
                # A VNDK APEX basename does not make a library visible to the
                # system-server classloader namespace. Prefer the platform file.
                if "Type: regular" in system.command("stat "+quote_debugfs("/"+relative)):
                    queue.append(relative)
                elif dependency not in available:
                    raise ValueError("missing platform native dependency: "+dependency)
        fragment=copied["system/etc/classpaths/systemserverclasspath.pb"]
        fragments=[fragment]+sorted(inputs.glob("system/apex/*/etc/classpaths/systemserverclasspath.pb"))
        classpaths={2:[],4:[]}
        for fragment in fragments:
            for tag,record in protobuf_fields(fragment.read_bytes()):
                if tag!=1 or not isinstance(record,bytes): raise ValueError("invalid server classpath record")
                fields={}
                for key,value in protobuf_fields(record):
                    if key not in (1,2,3,4) or key in fields: raise ValueError("ambiguous server classpath field")
                    fields[key]=value
                if fields.get(2) not in classpaths or not isinstance(fields.get(1),bytes): raise ValueError("unsupported server classpath kind")
                name=fields[1].decode()
                if not re.fullmatch(r"/(system/framework|apex/[A-Za-z0-9_.]+/javalib)/[A-Za-z0-9_.-]+[.]jar",name):raise ValueError("invalid server JAR")
                bounds=[fields.get(3,b""),fields.get(4,b"")]
                if any(not isinstance(v,bytes) or v and not re.fullmatch(rb"[0-9]{1,3}",v) for v in bounds):raise ValueError("unsupported server API bound")
                if not int(bounds[0] or b"0")<=33<=int(bounds[1] or b"999"):continue
                if name in classpaths[fields[2]]:raise ValueError("duplicate server JAR")
                classpaths[fields[2]].append(name)
                if name.startswith("/system/"):
                    target=temporary/"files"/name[1:]
                    system.dump(name,target);copied[name[1:]]=target
                elif not (inputs/("system"+name)).is_file(): raise ValueError("server JAR missing from selected APEX")
        for name,source in copied.items():
            target=inputs/name
            if name in metadata["symlinks"] or target.exists() and sha256(target)!=sha256(source):raise ValueError("conflicting platform boot file")
            target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target)
        for name,target in font_aliases.items():
            if name in metadata["files"] or name in metadata["symlinks"] and metadata["symlinks"][name]!=target:
                raise ValueError("conflicting platform font alias")
            metadata["symlinks"][name]=target
        for service,root in (("vold",VOLD_BINARY),("lmkd",LMKD_BINARY),("netd",NETD_BINARY)):
            if service not in metadata["profile"].get("host_services",[]):continue
            reachable=set();pending=[root]
            while pending:
                name=pending.pop()
                if name in reachable or name not in native_needed:continue
                reachable.add(name)
                pending.extend(dependency for dependency in native_needed[name] if dependency in native_needed)
            proof=metadata["host_service_inputs"][service]
            proof["files"]={name:sha256(copied[name]) for name in reachable}
            proof["native_needed"]={name:native_needed[name] for name in reachable}
        metadata["platform_boot"]={"sdk_sha256":SDK_SHA256,"partitions":partitions,"files":{name:sha256(source) for name,source in copied.items()},"classpaths":classpaths,"native_needed":native_needed}
    environment=inputs/"init.environ.rc"
    text="\n".join(line for line in environment.read_text().splitlines() if not line.startswith(("    export SYSTEMSERVERCLASSPATH ","    export STANDALONE_SYSTEMSERVER_JARS ")))+"\n"
    text=retain_stock_environment(text,environment_exports)
    for name,kind in [("SYSTEMSERVERCLASSPATH",2),("STANDALONE_SYSTEMSERVER_JARS",4)]:
        text+="    export "+name+" "+":".join(classpaths[kind])+"\n"
    environment.write_text(text)
    metadata["files"]={str(path.relative_to(inputs)):sha256(path) for path in sorted(inputs.rglob("*")) if path.is_file() and path.name!=MANIFEST}
    metadata["directories"]=[str(path.relative_to(inputs)) for path in sorted(inputs.rglob("*")) if path.is_dir()]
    (inputs/MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(inputs,runtime,stock)
    print("Merged verified platform service contexts and system-server JARs; startup has not been tested")


def merge_user_unlock(inputs,runtime,stock,build):
    from user_unlock_inputs import install
    metadata=verify_boot_inputs(inputs,runtime,stock)
    install(inputs,metadata,build)
    metadata["files"]={str(path.relative_to(inputs)):sha256(path) for path in sorted(inputs.rglob("*")) if path.is_file() and path.name!=MANIFEST}
    metadata["directories"]=[str(path.relative_to(inputs)) for path in sorted(inputs.rglob("*")) if path.is_dir()]
    (inputs/MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(inputs,runtime,stock)
    print("Merged verified Android user-0 unlock helper; unlock behavior has not been tested")


def merge_vold_subdirs(inputs,runtime,stock,build):
    from vold_inputs import install
    metadata=verify_boot_inputs(inputs,runtime,stock)
    install(inputs,metadata,build)
    metadata["files"]={str(path.relative_to(inputs)):sha256(path) for path in sorted(inputs.rglob("*")) if path.is_file() and path.name!=MANIFEST}
    metadata["directories"]=[str(path.relative_to(inputs)) for path in sorted(inputs.rglob("*")) if path.is_dir()]
    (inputs/MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(inputs,runtime,stock)
    print("Merged verified vold user-storage helper; vold execution has not been tested")


def parse_stock_environment(text):
    exports={}
    for line in text.splitlines():
        line=line.strip()
        if not line or line.startswith("#") or line=="on early-init":continue
        fields=line.split()
        if len(fields)!=3 or fields[0]!="export" or not re.fullmatch(r"[A-Z][A-Z0-9_]*",fields[1]) or not re.fullmatch(r"[A-Za-z0-9_./:+@-]+",fields[2]) or fields[1] in exports:
            raise ValueError("unexpected stock environment syntax")
        exports[fields[1]]=fields[2]
    return exports


def retain_stock_environment(text,exports):
    existing={match[1]:match[2] for match in re.finditer(r"^    export ([A-Z][A-Z0-9_]*) ([^\n]+)$",text,re.M)}
    for name,value in exports.items():
        if name in existing:
            if existing[name]!=value:raise ValueError("manager environment conflicts with stock: "+name)
        else:text+="    export "+name+" "+value+"\n"
    return text


def merge_angle(inputs,runtime,stock,archive):
    metadata=verify_boot_inputs(inputs,runtime,stock)
    available={Path(name).name for name in metadata["files"]}
    available|={Path(name).name for name in json.loads((runtime/"art-runtime.json").read_text())["files"]}
    libraries={};needed={}
    with tempfile.TemporaryDirectory(prefix="angle-inputs-",dir=inputs.parent) as directory:
        temporary=Path(directory);partitions=extract_partitions(archive,temporary)
        images={"system":Ext4(temporary/"system.ext4"),"vendor":Ext4(temporary/"vendor.ext4")}
        queue=["vendor/lib64/egl/libEGL_angle.so","vendor/lib64/egl/libGLESv1_CM_angle.so","vendor/lib64/egl/libGLESv2_angle.so"]
        seen=set()
        while queue:
            name=queue.pop()
            if name in seen:continue
            if len(seen)>=256:raise ValueError("ANGLE dependency bound")
            seen.add(name)
            partition,path=("vendor","/"+name.removeprefix("vendor/")) if name.startswith("vendor/") else ("system","/"+name)
            target=temporary/"files"/name;images[partition].dump(path,target)
            libraries[name]=target;available.add(Path(name).name)
            needed[name]=elf_needed(target)
            for dependency in needed[name]:
                if dependency in available:continue
                selected=None
                for part,original,destination in [("vendor","/lib64/egl/"+dependency,"vendor/lib64/egl/"+dependency),("vendor","/lib64/"+dependency,"vendor/lib64/"+dependency),("system","/system/lib64/"+dependency,"system/lib64/"+dependency)]:
                    if "Type: regular" in images[part].command("stat "+quote_debugfs(original)):
                        selected=destination;break
                if selected is None:raise ValueError("missing ANGLE dependency: "+dependency)
                queue.append(selected)
        for name,source in libraries.items():
            target=inputs/name
            if name in metadata["symlinks"] or target.exists() and sha256(target)!=sha256(source):raise ValueError("conflicting ANGLE library")
            target.parent.mkdir(parents=True,exist_ok=True)
            if not target.exists():shutil.copy2(source,target)
        metadata["egl_inputs"]={"sdk_sha256":SDK_SHA256,"driver":"angle","partitions":partitions,"files":{name:sha256(source) for name,source in libraries.items()},"needed":needed,"display_connection":"not implemented or asserted"}
    metadata["files"]={str(path.relative_to(inputs)):sha256(path) for path in sorted(inputs.rglob("*")) if path.is_file() and path.name!=MANIFEST}
    metadata["directories"]=[str(path.relative_to(inputs)) for path in sorted(inputs.rglob("*")) if path.is_dir()]
    (inputs/MANIFEST).write_text(json.dumps(metadata,indent=2)+"\n")
    verify_boot_inputs(inputs,runtime,stock)
    print("Merged verified SDK ANGLE libraries; EGL/display operation has not been tested")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--sdk-image", type=Path)
    source.add_argument("--reuse-payloads", type=Path)
    parser.add_argument("--vintf-image", type=Path,
                        help="pinned SDK image for changing framework declarations while reusing providers")
    parser.add_argument("--service-image",type=Path,help="pinned SDK image for adding the stock vold provider")
    parser.add_argument("--netd-firewall-compat",type=Path,help="verified Scarlet netd firewall compatibility build")
    parser.add_argument("--audio-start-build",type=Path,help="verified audioserver startup helper build")
    parser.add_argument("--audio-binder-compat",type=Path,help="verified audio HAL binder compatibility build")
    for flag in ["runtime-root", "stock-inputs", "profile", "output"]:
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--framework-inputs",type=Path,help="verified stock framework and app_process64 for init boot")
    parser.add_argument("--user-unlock-build",type=Path,help="verified Android user-0 unlock DEX build")
    parser.add_argument("--vold-subdirs-build",type=Path,help="verified vold user-storage helper build")
    parser.add_argument("--platform-image",type=Path,help="pinned SDK archive for stock service contexts and server JARs")
    parser.add_argument("--angle-image",type=Path,help="pinned SDK archive supplying its ANGLE libraries")
    for name in ["hal","adapters","client","vulkan"]:parser.add_argument("--graphics-"+name,type=Path,help="verified CPU graphics "+name+" build")
    args = parser.parse_args()
    if args.vintf_image and not args.reuse_payloads:
        parser.error("--vintf-image requires --reuse-payloads")
    if args.service_image and not args.reuse_payloads:
        parser.error("--service-image requires --reuse-payloads")
    graphics={name:getattr(args,"graphics_"+name) for name in ["hal","adapters","client"]}
    if (any(graphics.values()) or args.graphics_vulkan) and (not all(graphics.values()) or not args.platform_image):parser.error("graphics providers require all three builds and --platform-image")
    if args.graphics_vulkan:graphics["vulkan"]=args.graphics_vulkan
    if args.reuse_payloads:
        reconfigure(args.reuse_payloads, args.runtime_root, args.stock_inputs, args.profile, args.output, args.vintf_image, args.service_image, args.netd_firewall_compat, args.audio_start_build, args.audio_binder_compat)
    else:
        prepare(args.sdk_image, args.runtime_root, args.stock_inputs, args.profile, args.output)
    if args.framework_inputs:
        merge_framework(args.output,args.runtime_root,args.stock_inputs,args.framework_inputs)
    if args.user_unlock_build:
        merge_user_unlock(args.output,args.runtime_root,args.stock_inputs,args.user_unlock_build)
    if args.vold_subdirs_build:
        merge_vold_subdirs(args.output,args.runtime_root,args.stock_inputs,args.vold_subdirs_build)
    if args.platform_image:
        merge_platform(args.output,args.runtime_root,args.stock_inputs,args.platform_image,graphics if any(graphics.values()) else None)
    if args.angle_image:
        merge_angle(args.output,args.runtime_root,args.stock_inputs,args.angle_image)


if __name__ == "__main__":
    main()
