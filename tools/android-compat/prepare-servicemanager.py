#!/usr/bin/env python3
"""Extract the pinned stock servicemanager for direct startup diagnosis.

Run inside nix develop after prepare-art.py. This checks the existing ART
runtime's library closure; it does not add SELinux policy or certify services.
"""
import argparse
import json
from pathlib import Path
import tempfile

from art_inputs import Ext4, elf_needed, extract_system, sha256, verify_art_runtime


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-image", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    runtime, output = args.runtime_root.resolve(), args.output.resolve()
    verify_art_runtime(runtime)
    output.mkdir(parents=True, exist_ok=False)
    metadata = {"result": "PREPARING", "base_manifest_sha256": sha256(runtime / "art-runtime.json")}
    manifest = output / "service-inputs.json"
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    program = output / "system/bin/servicemanager"
    with tempfile.TemporaryDirectory(prefix="system-", dir=output) as temporary:
        image = Path(temporary) / "system.ext4"
        metadata["sdk"] = extract_system(args.sdk_image, image)
        Ext4(image).dump("/system/bin/servicemanager", program)
    search = [runtime / "system/lib64", runtime / "apex/com.android.runtime/lib64/bionic"]
    search += sorted((runtime / "apex").glob("*/lib64"))
    queue, dependencies, libraries = [program], {}, {}
    while queue:
        binary = queue.pop(0)
        name = str(binary.relative_to(output if binary == program else runtime))
        if name in dependencies:
            continue
        dependencies[name] = elf_needed(binary)
        for library in dependencies[name]:
            found = next((directory / library for directory in search if (directory / library).is_file()), None)
            if found is None:
                raise ValueError(f"servicemanager dependency absent from runtime: {library}")
            libraries[str(found.relative_to(runtime))] = sha256(found)
            queue.append(found)
    metadata.update({"result": "PREPARED", "execution": "not tested", "needed": dependencies,
                     "runtime_libraries": libraries, "files": {"system/bin/servicemanager": sha256(program)}})
    manifest.write_text(json.dumps(metadata, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
