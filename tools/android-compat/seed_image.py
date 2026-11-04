"""Keep Android inode metadata when staging a warm image through the host."""
import re
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path

from art_inputs import quote_debugfs


def stage_seed_data(source, target):
    """Replace generated data with one warm snapshot, preserving deletions too."""
    source, target = Path(source), Path(target)
    if source.is_symlink() or not source.is_dir():
        raise ValueError("seed data source is not a directory")
    if target.is_symlink() or target.exists() and not target.is_dir():
        raise ValueError("seed staging destination is not a directory")
    target.parent.mkdir(parents=True, exist_ok=True)
    # Complete the new copy before touching the previous generated tree. Both
    # renames stay on the staging filesystem, and publication can roll back.
    with tempfile.TemporaryDirectory(prefix=".scarlet-seed-", dir=target.parent) as directory:
        temporary = Path(directory)
        incoming, previous = temporary / "incoming", temporary / "previous"
        shutil.copytree(source, incoming, symlinks=True)
        if target.exists():
            target.rename(previous)
        try:
            incoming.rename(target)
        except OSError:
            if previous.exists():
                previous.rename(target)
            raise


_DEBUGFS_BANNER = re.compile(r"^debugfs \S+ \([^\n]+\)$")
_EA_ENTRY = re.compile(r"^  (.+) \(([0-9]+)\)(?: = .*)?$")
_EA_HEX = re.compile(r"^(?:[0-9a-fA-F]{2})*$")


def _debugfs(command, image):
    result = subprocess.run(
        ["debugfs", *command, str(image)],
        check=True, capture_output=True, text=True)
    errors = [line for line in (result.stderr or "").splitlines()
              if line and not _DEBUGFS_BANNER.fullmatch(line)]
    if errors:
        raise ValueError("debugfs failed: " + "; ".join(errors[:5]))
    return result


def _run_script(commands, image, temporary, name, writable=False):
    script = temporary / name
    script.write_text("\n".join(commands) + "\n")
    return _debugfs(["-w", "-f", str(script)] if writable else ["-f", str(script)], image)


def _list_extended_attributes(image, paths, temporary):
    """Return each inode's EA inventory without using stdout for values."""
    paths = sorted(paths)
    if not paths:
        return {}
    commands = ["ea_list " + quote_debugfs(path) for path in paths]
    result = _run_script(commands, image, temporary, "list-eas.debugfs")
    markers = {f"debugfs: ea_list {quote_debugfs(path)}": path for path in paths}
    groups = {}
    current = None
    for line in result.stdout.splitlines():
        if line in markers:
            current = markers[line]
            if current in groups:
                raise ValueError(f"duplicate debugfs EA listing: {current}")
            groups[current] = []
        elif current is not None:
            groups[current].append(line)
        elif line.strip():
            raise ValueError(f"unexpected debugfs EA output: {line}")
    if set(groups) != set(paths):
        missing = sorted(set(paths) - set(groups))
        raise ValueError("missing debugfs EA listing: " + ", ".join(missing[:3]))

    inventories = {}
    for path in paths:
        entries = {}
        for line in groups[path]:
            if not line or line == "Extended attributes:":
                continue
            match = _EA_ENTRY.fullmatch(line)
            if not match:
                raise ValueError(f"unrecognized debugfs EA entry for {path}: {line}")
            name, size = match.groups()
            if not name or name in entries:
                raise ValueError(f"duplicate or empty debugfs EA name for {path}: {name}")
            try:
                quote_debugfs(name)
            except ValueError as error:
                raise ValueError(f"invalid debugfs EA name for {path}") from error
            entries[name] = int(size)
        inventories[path] = entries
    return inventories


def _read_extended_attributes(image, paths, temporary):
    inventories = _list_extended_attributes(image, paths, temporary)
    reads = []
    values = {}
    for path in sorted(inventories):
        values[path] = {}
        for name, size in sorted(inventories[path].items()):
            output = temporary / f"ea-{len(reads)}"
            reads.append((path, name, size, output))
    commands = [
        f"ea_get -f {quote_debugfs(output)} {quote_debugfs(path)} {quote_debugfs(name)}"
        for path, name, _size, output in reads]
    if commands:
        _run_script(commands, image, temporary, "read-eas.debugfs")
    for path, name, expected_size, output in reads:
        if not output.is_file():
            raise ValueError(f"debugfs did not return EA {name!r} for {path}")
        value = output.read_bytes()
        if len(value) != expected_size:
            raise ValueError(
                f"truncated debugfs EA {name!r} for {path}: "
                f"expected {expected_size}, got {len(value)}")
        values[path][name] = value.hex()
    return values


def inode_metadata(image, root):
    """Read guest identities and extended attributes without host chown."""
    result = {}
    pending = [""]
    while pending:
        relative = pending.pop()
        path = root.rstrip("/") + ("/" + relative if relative else "")
        listing = _debugfs(["-R", "ls -p " + quote_debugfs(path)], image)
        found = False
        for row in listing.stdout.splitlines():
            if not row.strip():
                continue
            match = re.fullmatch(r"/(\d+)/([0-7]+)/(\d+)/(\d+)/([^/]*)/([0-9]*)/", row)
            if not match:
                raise ValueError(f"unrecognized seed inode: {row}")
            inode, mode, uid, gid, name, _size = match.groups()
            if int(inode) == 0 or name == "..":
                continue
            if not name:
                raise ValueError(f"empty live seed inode name: {row}")
            # rdump omits Unix sockets. Their endpoints belong to the previous
            # boot's processes; Android recreates them during this boot.
            if stat.S_ISSOCK(int(mode, 8)):
                continue
            key = relative if name == "." else (relative + "/" if relative else "") + name
            quote_debugfs(key)
            result[key] = {"mode": int(mode, 8), "uid": int(uid), "gid": int(gid)}
            if name == ".":
                found = True
            elif stat.S_ISDIR(int(mode, 8)):
                pending.append(key)
        if not found:
            raise ValueError(f"missing seed directory: {path}")

    def image_path(relative):
        return root.rstrip("/") + ("/" + relative if relative else "")

    relative_paths = {image_path(relative): relative for relative in result}
    with tempfile.TemporaryDirectory(prefix="scarlet-seed-eas-") as directory:
        attributes = _read_extended_attributes(
            image, relative_paths, Path(directory))
    for path, xattrs in attributes.items():
        relative = relative_paths[path]
        # Empty inventories stay absent so records from older helpers and
        # callers that compare those records remain backward compatible.
        if xattrs:
            result[relative]["xattrs"] = xattrs
    return result


def restore_inode_metadata(image, root, metadata):
    """Restore identities and extended attributes in the destination image."""
    with tempfile.TemporaryDirectory(prefix="scarlet-seed-inodes-") as directory:
        temporary = Path(directory)
        lines = []
        valued = {}
        for relative, fields in sorted(metadata.items()):
            path = root.rstrip("/") + ("/" + relative if relative else "")
            for field in ("uid", "gid", "mode"):
                value = fields[field]
                encoded = "0" + format(value, "o") if field == "mode" else str(value)
                lines.append(f"set_inode_field {quote_debugfs(path)} {field} {encoded}")
            if "xattrs" in fields:
                serialized = fields["xattrs"]
                if not isinstance(serialized, dict):
                    raise ValueError(f"seed xattrs are not an object: {root}/{relative}")
                desired = {}
                for name, value in serialized.items():
                    if not isinstance(name, str):
                        raise ValueError(f"seed EA name is not a string: {root}/{relative}")
                    quote_debugfs(name)
                    if not isinstance(value, str) or not _EA_HEX.fullmatch(value):
                        raise ValueError(
                            f"seed EA {name!r} is not valid strict hex: {root}/{relative}")
                    desired[name] = bytes.fromhex(value)
                valued[relative] = desired

        def image_path(relative):
            return root.rstrip("/") + ("/" + relative if relative else "")

        existing = _list_extended_attributes(
            image, [image_path(relative) for relative in valued], temporary)
        existing = {
            relative: existing[image_path(relative)] for relative in valued}
        value_number = 0
        for relative in sorted(valued):
            path = root.rstrip("/") + ("/" + relative if relative else "")
            desired = valued[relative]
            extras = sorted(set(existing[relative]) - set(desired))
            if extras:
                lines.append("ea_rm " + quote_debugfs(path) + " " +
                             " ".join(quote_debugfs(name) for name in extras))
            for name, value in sorted(desired.items()):
                value_file = temporary / f"value-{value_number}"
                value_file.write_bytes(value)
                value_number += 1
                lines.append(
                    f"ea_set -f {quote_debugfs(value_file)} "
                    f"{quote_debugfs(path)} {quote_debugfs(name)}")
        _run_script(lines, image, temporary, "restore-inodes.debugfs", writable=True)

    # debugfs can return success after a failed command. Read every value back.
    actual = inode_metadata(image, root)
    for path, fields in metadata.items():
        if path not in actual:
            raise ValueError(f"seed inode was not preserved: {root}/{path}")
        for field in ("mode", "uid", "gid"):
            if actual[path][field] != fields[field]:
                raise ValueError(f"seed {field} was not preserved: {root}/{path}")
        if "xattrs" in fields and actual[path].get("xattrs", {}) != fields["xattrs"]:
            raise ValueError(f"seed extended attributes were not preserved: {root}/{path}")
