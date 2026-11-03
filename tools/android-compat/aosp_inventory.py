"""Read-only inventory of init definitions in complete, hash-pinned images.

Declarations are not a resolved boot graph: imports/property expansion, service
overrides and runtime Binder lookups still require source-level review.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import stat
import subprocess
import tempfile

from aosp_images import verify
from art_inputs import quote_debugfs, sha256, unpack_apex
from stock_art import PARTITIONS, apex_identity


def init_lines(text):
    """Tokenize init syntax, including literal multiline double-quoted values.

    AOSP init/tokenizer.cpp: comments begin at token boundaries; quotes do not
    interpret escapes; unquoted backslash-newline joins physical lines.
    Locations here count physical source lines, including quoted newlines.
    """
    words, token = [], []
    active = False
    line = start = 1
    pos = 0
    while pos < len(text):
        char = text[pos]
        if char == "#" and not active:
            end = text.find("\n", pos)
            pos = len(text) if end < 0 else end
            continue
        if char in " \t\r\n":
            if active:
                words.append("".join(token))
                token, active = [], False
            if char == "\n":
                if words:
                    yield start, words
                words = []
                line += 1
                start = line
            pos += 1
            continue
        if not words and not active:
            start = line
        active = True
        if char == '"':
            end = text.find('"', pos + 1)
            if end < 0:
                raise ValueError(f"unterminated init quote at line {line}")
            quoted = text[pos + 1:end]
            token.append(quoted)
            line += quoted.count("\n")
            pos = end + 1
        elif char == "\\":
            pos += 1
            if pos == len(text):
                raise ValueError("unterminated init continuation")
            if text[pos:pos + 2] == "\r\n":
                pos += 1
            if text[pos] == "\n":
                pos += 1
                line += 1
                while pos < len(text) and text[pos] in " \t":
                    pos += 1
            else:
                token.append({"n": "\n", "t": "\t", "r": "\r"}.get(text[pos], text[pos]))
                pos += 1
        else:
            token.append(char)
            pos += 1
    if active:
        words.append("".join(token))
    if words:
        yield start, words


def parse_rc(text):
    """Retain every declaration and command; do not select services to start."""
    sections = []
    current = None
    for start, words in init_lines(text):
        if words[0] in ("service", "on", "import"):
            if len(words) < (3 if words[0] == "service" else 2):
                raise ValueError(f"incomplete init declaration at line {start}")
            current = {"line": start, "header": words, "commands": []}
            sections.append(current)
        elif current is None or current["header"][0] == "import":
            raise ValueError(f"command outside init section at line {start}")
        else:
            current["commands"].append({"line": start, "args": words})
    return sections


class Image:
    def __init__(self, path, debugfs):
        self.path, self.debugfs = path, debugfs

    def command(self, command):
        result = subprocess.run([self.debugfs, "-R", command, str(self.path)],
                                check=True, capture_output=True)
        # debugfs reports filesystem/command failures on stderr with status 0.
        errors = [line for line in result.stderr.decode().splitlines()
                  if line and not line.startswith("debugfs ")]
        if errors:
            raise ValueError(f"{self.path}: {command}: {'; '.join(errors)}")
        return result.stdout

    def entries(self, path):
        result = []
        for line in self.command("ls -p " + quote_debugfs(path)).decode().splitlines():
            if not line.strip():
                continue
            match = re.fullmatch(r"/(\d+)/([0-7]+)/(\d+)/(\d+)/([^/]*)/([0-9]*)/", line)
            if not match:
                raise ValueError(f"invalid directory listing: {line}")
            if int(match[1]) and match[5] not in (".", ".."):
                result.append((match[5], int(match[2], 8)))
        return result

    def configs(self, root):
        for name, mode in self.entries(root):
            path = root.rstrip("/") + "/" + name
            if stat.S_ISDIR(mode):
                yield from self.configs(path)
            elif stat.S_ISREG(mode) and name.endswith(".rc"):
                yield path, self.command("cat " + quote_debugfs(path))
            elif stat.S_ISLNK(mode):
                raise ValueError(f"init directory link requires explicit resolution: {path}")

    def dump(self, source, target):
        self.command(f"dump {quote_debugfs(source)} {quote_debugfs(target)}")
        if not target.is_file():
            raise ValueError(f"missing dumped file: {source}")


def extent_hash(path, offset, size):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        source.seek(offset)
        while size:
            block = source.read(min(size, 1024 * 1024))
            if not block:
                raise ValueError("truncated partition extent")
            digest.update(block)
            size -= len(block)
    return digest.hexdigest()


def inventory(images, partitions, debugfs):
    inputs = verify(images)
    result = {"kind": "scarlet-aosp-init-inventory-v1", "sdk_sha256": inputs["sdk_sha256"],
              "boot_validation": "not run", "coverage": "init declarations, not runtime reachability",
              "partitions": {}, "configs": {}, "apexes": [], "fstabs": {}}

    def add_config(source, data):
        try:
            sections = parse_rc(data.decode())
        except ValueError as error:
            raise ValueError(f"{source}: {error}") from error
        result["configs"][source] = {"sha256": hashlib.sha256(data).hexdigest(),
                                     "sections": sections}

    for name, (sector, sectors) in PARTITIONS.items():
        path = partitions / (name + ".ext4")
        offset, size = (4096 + sector) * 512, sectors * 512
        expected = extent_hash(images / "system.img", offset, size)
        if path.stat().st_size != size or sha256(path) != expected:
            raise ValueError(f"partition differs from pinned SDK: {name}")
        result["partitions"][name] = {"sha256": expected, "offset": offset, "size": size}
        image = Image(path, debugfs)
        root = "/system" if name == "system" else ""
        if name == "system":
            for entry, mode in image.entries("/"):
                if entry.endswith(".rc") and stat.S_ISREG(mode):
                    add_config(name + ":/" + entry,
                               image.command("cat " + quote_debugfs("/" + entry)))
        etc = root + "/etc"
        entries = dict(image.entries(etc))
        if "init" in entries:
            for source, data in image.configs(etc + "/init"):
                add_config(name + ":" + source, data)
        for entry, mode in entries.items():
            if entry.startswith("fstab.") and stat.S_ISREG(mode):
                source = etc + "/" + entry
                data = image.command("cat " + quote_debugfs(source))
                result["fstabs"][name + ":" + source] = {
                    "sha256": hashlib.sha256(data).hexdigest(), "text": data.decode()}
        if "apex" not in dict(image.entries(root or "/")):
            continue
        for entry, mode in image.entries(root + "/apex"):
            if not entry.endswith((".apex", ".capex")):
                continue
            if not stat.S_ISREG(mode):
                raise ValueError("APEX package is not a regular file")
            source = root + "/apex/" + entry
            with tempfile.TemporaryDirectory(prefix="scarlet-apex-inventory-") as directory:
                temporary = Path(directory)
                package, payload = temporary / entry, temporary / "payload.ext4"
                image.dump(source, package)
                unpack_apex(package, payload)
                apex = Image(payload, debugfs)
                identity = apex_identity(apex.command("cat /apex_manifest.pb"))
                result["apexes"].append({"source": name + ":" + source, "name": identity[0],
                    "version": identity[1], "package_sha256": sha256(package),
                    "payload_sha256": sha256(payload)})
                # apexd imports /apex/<module>/etc/*.rc, not only etc/init/.
                if "etc" in dict(apex.entries("/")):
                    for config, data in apex.configs("/etc"):
                        add_config(name + ":" + source + "!" + config, data)
    result["service_count"] = sum(section["header"][0] == "service"
        for config in result["configs"].values() for section in config["sections"])
    return result


def fixture_inventory(profile_path, image_path, debugfs):
    """Inspect actual saved fixture bytes, not just its intended configuration."""
    profile = json.loads(profile_path.read_text())
    image = Image(image_path, debugfs)
    result = {"profile_sha256": sha256(profile_path), "configs": {}, "other_rc": {},
              "coverage": "rc paths recorded in the saved fixture manifest"}
    for path, expected in profile["files"].items():
        if not path.endswith(".rc"):
            continue
        data = image.command("cat " + quote_debugfs("/systems/android-aarch64/" + path))
        actual = hashlib.sha256(data).hexdigest()
        if actual != expected:
            raise ValueError(f"saved fixture changed since manifest: {path}")
        # ueventd has a different grammar. Preserve its identity, without
        # misinterpreting device rules as init actions.
        if Path(path).name == "ueventd.rc":
            result["other_rc"][path] = actual
        else:
            result["configs"][path] = {"sha256": actual, "sections": parse_rc(data.decode())}
    result["service_count"] = sum(section["header"][0] == "service"
        for config in result["configs"].values() for section in config["sections"])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--images", type=Path, required=True)
    parser.add_argument("--partitions", type=Path, required=True)
    parser.add_argument("--debugfs", default="debugfs")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fixture-profile", type=Path)
    parser.add_argument("--fixture-rootfs", type=Path)
    args = parser.parse_args()
    if bool(args.fixture_profile) != bool(args.fixture_rootfs):
        parser.error("fixture comparison requires both profile and rootfs")
    if args.output.exists():
        raise ValueError("refusing to replace existing inventory")
    result = inventory(args.images, args.partitions, args.debugfs)
    if args.fixture_profile:
        result["fixture"] = fixture_inventory(args.fixture_profile, args.fixture_rootfs, args.debugfs)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as target:
        json.dump(result, target, indent=2)
        target.write("\n")
    print(f"Read {len(result['configs'])} init files, {len(result['apexes'])} APEX packages, "
          f"{result['service_count']} service declarations; no boot asserted")


if __name__ == "__main__":
    main()
