"""Snapshot the changed CK3 script files under one Git working tree directory."""

import codecs
import contextlib
import hashlib
import os
from pathlib import Path, PureWindowsPath
import re
import stat
import subprocess
import tempfile


def _git_command(root, *args):
    return ["git", "--no-optional-locks", "--literal-pathspecs",
            "-c", "core.fsmonitor=false", "-C", root, *args]


def _git(root, *args):
    result = subprocess.run(_git_command(root, *args), capture_output=True,
                            env={**os.environ, "GIT_NO_LAZY_FETCH": "1"})
    if result.returncode:
        raise ValueError(result.stderr.decode(errors="replace").strip() or "Git command failed")
    return result.stdout


def _inventory(root, base):
    # Both sides of moves remain visible, including moves across the mod boundary.
    raw = _git(root, "diff", "--raw", "--no-abbrev", "--relative", "--no-renames",
               "--no-ext-diff", "--no-textconv", "--ignore-submodules=none", "-z", base, "--", ".")
    fields = iter(raw.split(b"\0")[:-1])
    entries = {}
    for header in fields:
        old_mode, new_mode, old_oid, _, status = header.removeprefix(b":").split()
        path = os.fsdecode(next(fields))
        entries[path] = (old_mode, new_mode, old_oid.decode("ascii"), status)

    untracked = _git(root, "ls-files", "--others", "--exclude-standard", "-z", "--", ".")
    for raw_path in untracked.split(b"\0")[:-1]:
        path = os.fsdecode(raw_path)
        # A staged removal can be followed by a new, untracked file at the same path.
        old_mode, _, oid, _ = entries.get(path, (b"000000", None, None, None))
        entries[path] = (old_mode, None, oid, b"?")

    unmerged = _git(root, "ls-files", "--unmerged", "-z", "--", ".")
    conflicts = {os.fsdecode(row.split(b"\t", 1)[1]) for row in unmerged.split(b"\0")[:-1]}
    return entries, conflicts


def _skip_reason(path, modes, status):
    parts = path.split("/")
    if not path or any(part in ("", ".", "..") or part.lower() == ".git" for part in parts):
        return "unsupported directory or unsafe Git path"
    if PureWindowsPath(path).drive or (os.name == "nt" and
            ("\\" in path or any(_reserved_windows_component(part) for part in parts))):
        return "Git path cannot be safely opened here"
    if parts[0] not in ("common", "events") or len(parts) < 2 or not path.endswith(".txt"):
        return "outside supported CK3 script paths (common/ and events/ .txt files)"
    if status not in (b"A", b"M", b"D", b"T", b"?"):
        return "unsupported Git change type " + status.decode("ascii", errors="replace")
    if any(mode not in (None, b"000000", b"100644", b"100755") for mode in modes):
        return "Git symlink, submodule, or other non-regular entry"
    return None


def _reserved_windows_component(part):
    # Python 3.11 supports this runner and predates os.path.isreserved.
    if hasattr(os.path, "isreserved"):
        return os.path.isreserved(part)
    return (PureWindowsPath(part).is_reserved() or part.endswith((" ", ".")) or
            any(ord(char) < 32 or char in '<>:"|?*' for char in part))


def _copy_script(stream, destination, remaining=None, digest=None):
    """Consume the whole blob even if it cannot be analyzed, preserving batch framing."""
    decoder = codecs.getincrementaldecoder("utf-8")()
    reason = None
    first = True
    with destination.open("xb") as output:
        while remaining != 0:
            limit = min(remaining, 65536) if remaining is not None else 65536
            chunk = stream.read(limit)
            if not chunk:
                if remaining is not None:
                    raise ValueError("Git returned a truncated blob")
                break
            if remaining is not None:
                remaining -= len(chunk)
            if digest is not None:
                digest.update(chunk)
            if first and chunk.startswith(b"version https://git-lfs.github.com/spec/v1"):
                reason = "Git LFS pointer in place of script content"
            first = False
            if not reason:
                if b"\0" in chunk:
                    reason = "binary or UTF-16 content"
                else:
                    try:
                        decoder.decode(chunk)
                    except UnicodeDecodeError:
                        reason = "script content is not UTF-8"
            if not reason:
                output.write(chunk)
        if not reason:
            try:
                decoder.decode(b"", final=True)
            except UnicodeDecodeError:
                reason = "script content is not UTF-8"
    if reason:
        destination.unlink()
    return reason


def _historical_files(root, oids, scratch):
    result = {}
    if not oids:
        return result
    # One process for all blobs. Requesting each object before reading its response
    # keeps memory bounded even with a large diff, and avoids filling both pipes.
    with tempfile.TemporaryFile() as errors:
        with subprocess.Popen(_git_command(root, "cat-file", "--batch"), stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=errors,
                              env={**os.environ, "GIT_NO_LAZY_FETCH": "1"}) as proc:
            try:
                for oid in sorted(oids):
                    proc.stdin.write(oid.encode("ascii") + b"\n")
                    proc.stdin.flush()
                    header = proc.stdout.readline().split()
                    if header == [oid.encode("ascii"), b"missing"]:
                        result[oid] = (None, "historical Git blob is unavailable")
                        continue
                    if len(header) != 3 or header[:2] != [oid.encode("ascii"), b"blob"]:
                        raise ValueError("unexpected response while reading historical Git blobs")
                    destination = scratch / f"before-{oid}.txt"
                    reason = _copy_script(proc.stdout, destination, int(header[2]))
                    if proc.stdout.read(1) != b"\n":
                        raise ValueError("invalid historical Git blob boundary")
                    result[oid] = (str(destination), reason)
            finally:
                proc.stdin.close()
                # An interrupted disk write can leave Git blocked writing the rest of a blob.
                proc.stdout.close()
            if proc.wait():
                errors.seek(0)
                raise ValueError(errors.read().decode(errors="replace").strip() or "git cat-file failed")
    return result


def _link(info):
    return stat.S_ISLNK(info.st_mode) or bool(
        getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT)


def _file_state(info):
    # Windows can expose different ctime semantics on a path and on its open handle.
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns)


class _Unreadable(ValueError):
    pass


@contextlib.contextmanager
def _open_working_file(root, path):
    file = root.joinpath(*path.split("/"))
    try:
        for parent in reversed(file.relative_to(root).parents):
            if parent.parts and _link((root / parent).lstat()):
                raise _Unreadable("working path traverses a symlink or junction")
        original = file.lstat()
        if _link(original) or not stat.S_ISREG(original.st_mode):
            raise _Unreadable("working file is not a regular file")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0)
        handle = os.open(file, flags)
    except FileNotFoundError as exc:
        raise _Unreadable("working file disappeared while collecting changes") from exc
    except PermissionError as exc:
        raise _Unreadable("working file could not be read") from exc
    with os.fdopen(handle, "rb") as source:
        state = _file_state(os.fstat(source.fileno()))
        if state != _file_state(original):
            raise ValueError("working file changed while opening it: " + path)
        yield source
        try:
            stable = state == _file_state(os.fstat(source.fileno())) == _file_state(file.lstat())
        except FileNotFoundError:
            stable = False
        if not stable:
            raise ValueError("working file changed while reading it: " + path)


class _Inventory(dict):
    # Keep verification state out of the public, JSON-serializable report.
    __slots__ = ("repo_root", "git_state", "working_state")


def collect(root: Path, revision: str, work: Path) -> dict:
    """Compare a pinned commit to the working tree, with paths relative to the mod root."""
    root = Path(root).resolve(strict=True)
    work = Path(work).resolve()
    if work.is_relative_to(root):
        raise ValueError("impact snapshot directory must be outside the mod being compared")
    if not revision or "\0" in revision:
        raise ValueError("a Git commit revision is required")
    base = _git(root, "rev-parse", "--verify", "--end-of-options", f"{revision}^{{commit}}").strip().decode("ascii")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", base):
        raise ValueError("Git did not return a full commit object ID")

    entries, conflicts = _inventory(root, base)
    skipped = {path: "unresolved Git index conflict" for path in conflicts}
    candidates = {}
    for path, (old_mode, new_mode, oid, status) in entries.items():
        if path in skipped:
            continue
        reason = _skip_reason(path, (old_mode, new_mode), status)
        if reason:
            skipped[path] = reason
        else:
            candidates[path] = (oid if old_mode != b"000000" else None, new_mode != b"000000")

    work.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="impact-", dir=work))
    old = _historical_files(root, {oid for oid, _ in candidates.values() if oid}, scratch)
    files = []
    working_state = {}
    for index, path in enumerate(sorted(candidates)):
        oid, present = candidates[path]
        before, reason = old.get(oid, (None, None))
        after = None
        if present:
            destination = scratch / f"after-{index}.txt"
            try:
                with _open_working_file(root, path) as source:
                    digest = hashlib.sha256()
                    current_reason = _copy_script(source, destination, digest=digest)
                working_state[path] = digest.digest()
                after = str(destination)
            except _Unreadable as exc:
                current_reason = str(exc)
                working_state[path] = current_reason
            reason = reason or current_reason
        if reason:
            skipped[path] = reason
        else:
            files.append({"path": path, "before": before, "after": after})

    result = _Inventory(base=base, files=files,
                        skipped=[{"path": path, "reason": skipped[path]} for path in sorted(skipped)])
    result.repo_root = root
    result.git_state = entries, conflicts
    result.working_state = working_state
    if result.git_state != _inventory(root, base):
        raise ValueError("Git index or working tree changed during impact collection, retry")
    return result


def verify(root: Path, inventory: dict) -> None:
    """Verify the original collect() result against the working tree after validation."""
    root = Path(root).resolve(strict=True)
    if not isinstance(inventory, _Inventory) or inventory.repo_root != root:
        raise ValueError("impact verification requires the original inventory for this mod")
    for path, expected in inventory.working_state.items():
        try:
            with _open_working_file(root, path) as source:
                current = hashlib.file_digest(source, "sha256").digest()
        except _Unreadable as exc:
            current = str(exc)
        if current != expected:
            raise ValueError("working file changed during impact validation: " + path)
    if inventory.git_state != _inventory(root, inventory["base"]):
        raise ValueError("Git changed paths or statuses during impact validation, retry")
