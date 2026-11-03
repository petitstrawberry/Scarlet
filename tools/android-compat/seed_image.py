"""Keep Android inode ownership when staging a warm image through the host."""
import re
import stat
import subprocess
import tempfile
from pathlib import Path

from art_inputs import quote_debugfs


def inode_metadata(image, root):
    """Read guest identities directly; rdump cannot chown to Android UIDs on macOS."""
    result = {}
    pending = [""]
    while pending:
        relative = pending.pop()
        path = root.rstrip("/") + ("/" + relative if relative else "")
        listing = subprocess.run(
            ["debugfs", "-R", "ls -p " + quote_debugfs(path), str(image)],
            check=True, capture_output=True, text=True)
        found = False
        for row in listing.stdout.splitlines():
            if not row.strip():
                continue
            match = re.fullmatch(r"/(\d+)/([0-7]+)/(\d+)/(\d+)/([^/]+)/([0-9]*)/", row)
            if not match:
                raise ValueError(f"unrecognized seed inode: {row}")
            inode, mode, uid, gid, name, _size = match.groups()
            if int(inode) == 0 or name == "..":
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
    return result


def restore_inode_metadata(image, root, metadata):
    """Restore identities in the destination image, without privileged host chown."""
    with tempfile.TemporaryDirectory(prefix="scarlet-seed-inodes-") as temporary:
        commands = Path(temporary) / "inodes.txt"
        lines = []
        for relative, fields in sorted(metadata.items()):
            path = root.rstrip("/") + ("/" + relative if relative else "")
            for field in ("uid", "gid", "mode"):
                value = fields[field]
                encoded = "0" + format(value, "o") if field == "mode" else str(value)
                lines.append(f"set_inode_field {quote_debugfs(path)} {field} {encoded}")
        commands.write_text("\n".join(lines) + "\n")
        subprocess.run(["debugfs", "-w", "-f", str(commands), str(image)],
                       check=True, capture_output=True, text=True)
    # debugfs can return success after a failed command. Read the image back.
    actual = inode_metadata(image, root)
    for path, fields in metadata.items():
        if actual.get(path) != fields:
            raise ValueError(f"seed ownership/mode was not preserved: {root}/{path}")
