#!/usr/bin/env python3
"""Add a local console account to a mounted Scarlet root filesystem."""

import argparse
import fcntl
import getpass
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile


def identity(value):
    if not value.isascii() or not value.isdecimal() or not 0 < int(value) < 2**32 - 1:
        raise ValueError("expected a nonzero 32-bit identity")
    return int(value)


def protected_dir(path):
    mode = path.lstat()
    if not stat.S_ISDIR(mode.st_mode) or mode.st_uid != 0 or mode.st_mode & 0o022:
        raise ValueError(f"{path} must be a root-owned directory writable only by root")


def add_user(root, name, uid, gid, groups, verifier):
    if root == Path("/") or not (root / "bin/login").is_file():
        raise ValueError("select a mounted Scarlet root filesystem")
    protected_dir(root)
    etc = root / "etc"
    protected_dir(etc)
    homes = root / "home"
    homes.mkdir(mode=0o755, exist_ok=True)
    protected_dir(homes)

    # The directory is stable across database replacements. Lock writers before
    # checking uniqueness or creating a home shared with any other account.
    lock = os.open(etc, os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX)
        db = etc / "scarlet-users"
        try:
            info = db.lstat()
        except FileNotFoundError:
            data = b""
        else:
            if not stat.S_ISREG(info.st_mode) or info.st_uid or info.st_mode & 0o077:
                raise ValueError("existing account database must be a root-only regular file")
            with db.open("rb") as source:
                data = source.read(65537)
        for row in data.decode().splitlines():
            login, existing_uid, _, _, _ = row.split(":")
            if login == name or identity(existing_uid) == uid:
                raise ValueError("account name or UID already in use")
        if data and not data.endswith(b"\n"):
            data += b"\n"
        data += f"{name}:{uid}:{gid}:{','.join(map(str, groups))}:{verifier}\n".encode()
        if len(data) > 65536:
            raise ValueError("account database exceeds 64 KiB")

        home = homes / name
        descriptor, pending = tempfile.mkstemp(dir=etc)
        try:
            with os.fdopen(descriptor, "wb") as target:
                target.write(data)
                target.flush()
                os.fsync(target.fileno())
            # Stage ownership before publishing the name. An interrupted write
            # may already have installed the matching empty home.
            if home.exists() or home.is_symlink():
                info = home.lstat()
                if (not stat.S_ISDIR(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o700
                        or (info.st_uid, info.st_gid) != (uid, gid) or any(home.iterdir())):
                    raise ValueError("home directory already in use")
            else:
                staged = Path(tempfile.mkdtemp(dir=homes))
                try:
                    os.chown(staged, uid, gid)
                    staged.rename(home)
                finally:
                    if staged.exists():
                        staged.rmdir()
            home_fd = os.open(homes, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(home_fd)
            finally:
                os.close(home_fd)
            os.replace(pending, db)
            os.fsync(lock)
        finally:
            Path(pending).unlink(missing_ok=True)
    finally:
        os.close(lock)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--uid", type=identity, required=True)
    parser.add_argument("--gid", type=identity)
    parser.add_argument("--group", type=identity, action="append", default=[])
    parser.add_argument("name")
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z][a-z0-9_-]{0,31}", args.name):
        parser.error("invalid account name")
    if os.geteuid() != 0:
        parser.error("run as root to assign ownership of the home directory")
    password = getpass.getpass("New password: ").encode()
    if not 1 <= len(password) <= 72 or any(b in password for b in (0, 10, 13)):
        parser.error("enter 1 to 72 password bytes without NUL or line endings")
    if password != getpass.getpass("Retype password: ").encode():
        parser.error("passwords differ")
    result = subprocess.run(["htpasswd", "-niBC", "12", args.name],
                            input=password + b"\n", stdout=subprocess.PIPE, check=True)
    verifier = result.stdout.decode().strip().split(":", 1)[1]
    if not re.fullmatch(r"\$2[by]\$12\$[./A-Za-z0-9]{53}", verifier):
        parser.error("htpasswd returned an unsupported verifier")
    add_user(args.root.resolve(), args.name, args.uid, args.gid or args.uid,
             sorted(set(args.group)), verifier)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error))
