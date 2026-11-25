"""Validate a CK3 mod, compare revisions, or look up an identifier in the loaded game and mods."""

import argparse
from collections import Counter
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BINARY = ROOT / "target/release" / ("ck3-tiger.exe" if os.name == "nt" else "ck3-tiger")
SEVERITIES = ("tips", "untidy", "warning", "error", "fatal")
CACHE_FORMAT = 1


def directory(value):
    path = Path(value).resolve(strict=True)
    if not path.is_dir():
        raise ValueError(f"Expected a directory: {path}")
    return path


def quoted(value):
    value = str(value)
    if any(c in value for c in '"\r\n'):
        raise ValueError(f"Cannot quote path in a Tiger configuration: {value!r}")
    return f'"{value}"'


def descriptor(root, work, label):
    original = root / "descriptor.mod"
    if original.is_file():
        return original
    # Bare drop-in trees have no launcher descriptor or replace_path directives.
    temporary = work / f"{label}.mod"
    temporary.write_text(f'name="{label}"\npath={quoted(root.as_posix())}\n', encoding="utf-8")
    return temporary


def configuration(work, parents, show_parents, show_vanilla):
    config = work / "runner.conf"
    settings = [
        'languages = { check = "english" }',
        'filter = { show_loaded_mods = %s show_vanilla = %s }' % (
            "yes" if show_parents else "no", "yes" if show_vanilla else "no"),
    ]
    for index, parent in enumerate(parents):
        modfile = descriptor(parent, work, f"parent-{index}")
        settings.append(f'load_mod = {{ label = "parent-{index}" modfile = {quoted(modfile.as_posix())} }}')
    config.write_text("\n".join(settings) + "\n", encoding="utf-8")
    return config


def report_identity(report):
    # A line insertion moves a finding. Its source text, owner and caller chain still identify it.
    locations = [(p.get("from"), p["path"].replace("\\", "/"), (p.get("line") or "").strip(),
                  p.get("tag"), p.get("stage")) for p in report["locations"]]
    identity = [report["severity"], report["confidence"], report["key"], report["message"],
                report.get("info"), locations]
    return hashlib.sha256(json.dumps(identity, ensure_ascii=False).encode()).hexdigest()


def git_revision(root):
    try:
        result = subprocess.run(["git", "--no-optional-locks", "-C", root, "rev-parse", "--verify", "HEAD"],
                                capture_output=True, text=True)
    except FileNotFoundError:
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def input_revision(label, root):
    """Identify the worktree containing this input, including nested mod directories."""
    entry = dict(label=label, path=str(root), repository=None, revision=None, dirty=None)

    def git(*args):
        return subprocess.run(["git", "--no-optional-locks", "-C", root, *args],
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)

    try:
        top = git("rev-parse", "--show-toplevel")
    except FileNotFoundError:
        return entry
    if top.returncode:
        return entry
    repo = Path(os.fsdecode(top.stdout.rstrip(b"\r\n"))).resolve()
    entry["repository"] = str(repo)
    revision = git("rev-parse", "--verify", "HEAD")
    if revision.returncode == 0:
        entry["revision"] = revision.stdout.decode("ascii").strip()
    # An ignored drop-in under a repo (e.g. a build directory) cannot be called clean.
    if root != repo and git("check-ignore", "--quiet", "--", ".").returncode == 0:
        return entry
    status = git("status", "--porcelain=v1", "-z", "--untracked-files=normal", "--", ".")
    if status.returncode == 0:
        entry["dirty"] = bool(status.stdout)
    return entry


def json_rows(stream):
    """Read Tiger's JSON array incrementally; a truncated or malformed report is a tool failure."""
    decoder = json.JSONDecoder()
    buffer = ""

    def next_token():
        nonlocal buffer
        buffer = buffer.lstrip()
        while not buffer:
            chunk = stream.read(65536)
            if not chunk:
                break
            buffer = chunk.lstrip()
        return buffer[:1]

    if next_token() != "[":
        raise ValueError("Tiger did not return a JSON array")
    buffer = buffer[1:]
    if next_token() != "]":
        while True:
            while True:
                try:
                    row, end = decoder.raw_decode(buffer)
                    break
                except json.JSONDecodeError:
                    chunk = stream.read(65536)
                    if not chunk:
                        raise
                    buffer += chunk
            if (not isinstance(row, dict) or row.get("severity") not in SEVERITIES
                    or not all(isinstance(row.get(k), str) for k in ("confidence", "key", "message"))
                    or not isinstance(row.get("locations"), list)
                    or any(not isinstance(p, dict) or not isinstance(p.get("path"), str)
                           for p in row["locations"])):
                raise ValueError("Malformed Tiger diagnostic")
            yield row
            buffer = buffer[end:]
            token = next_token()
            if token == "]":
                break
            if token != ",":
                raise ValueError("Invalid separator in Tiger output")
            buffer = buffer[1:]
            if next_token() != "{":
                raise ValueError("Expected a diagnostic after the comma")
    buffer = buffer[1:]
    if next_token():
        raise ValueError("Unexpected data after Tiger's report")


@contextmanager
def tiger_command(tiger, game, mod, parents, *, stable, show_parents, show_vanilla, symbol=None):
    with tempfile.TemporaryDirectory(prefix="ck3-tiger-") as folder:
        work = Path(folder)
        conf = configuration(work, parents, show_parents, show_vanilla)
        command = [tiger, "--game", game, "--config", conf, "--json", "--no-color", "--pod"]
        if stable:
            command.append("--deterministic")
        if symbol is not None:
            command.extend(["--lookup", symbol])
        command.append(descriptor(mod, work, "target"))
        yield command


def scan(tiger, game, mod, parents, **settings):
    with tiger_command(tiger, game, mod, parents, **settings) as command:
        with tempfile.TemporaryFile("w+", encoding="utf-8") as output:
            result = subprocess.run(command, stdout=output, stderr=subprocess.PIPE)
            if result.returncode:
                raise ValueError(f"Tiger failed ({result.returncode}):\n"
                                 + result.stderr.decode("utf-8", errors="replace"))
            output.seek(0)
            yield from json_rows(output)


def lookup(tiger, game, mod, parents, symbol, **settings):
    with tiger_command(tiger, game, mod, parents, symbol=symbol, **settings) as command:
        result = subprocess.run(command, capture_output=True, encoding="utf-8")
    if result.returncode:
        raise ValueError(f"Tiger lookup failed ({result.returncode}):\n{result.stderr}")
    found = json.loads(result.stdout)
    if (not isinstance(found, dict) or found.get("symbol") != symbol
            or not isinstance(found.get("definitions"), list)
            or not isinstance(found.get("callers"), list)):
        raise ValueError("Malformed Tiger lookup result")
    return found


def print_lookup(found):
    def location(source):
        return f'{source["from"]} {source["path"]}:{source["line"]}'

    print(found["symbol"])
    for definition in sorted(found["definitions"], key=lambda row: row["status"] == "shadowed"):
        parameters = ", ".join(definition.get("parameters") or [])
        suffix = f" ({parameters})" if parameters else ""
        reason = f' [{definition["reason"]}]' if definition.get("reason") else ""
        print(f'  {definition["status"]} {definition["kind"]}{suffix}  {location(definition)}{reason}')
    if not found["definitions"]:
        print("  No indexed definition")
    for site in found["callers"]:
        print(f'  {site["kind"]} reference in {site.get("caller") or "(unknown caller)"}  {location(site)}')
        for call in site.get("expansion", []):
            print(f'    expanded at {location(call)}')
    if not found["callers"]:
        print("  No resolved callers")
    if found.get("caller_coverage"):
        print(found["caller_coverage"])


def baseline_key(metadata, roots):
    digest = hashlib.sha256(json.dumps(metadata, sort_keys=True).encode())
    for root in roots:
        digest.update(os.fsencode(root) + b"\0")
        digest.update((git_revision(root) or "").encode() + b"\0")
        pending = [root]
        while pending:
            folder = pending.pop()
            with os.scandir(folder) as entries:
                for entry in sorted(entries, key=lambda e: e.name):
                    if folder == root and entry.name == ".git":
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        pending.append(Path(entry.path))
                    elif entry.is_file(follow_symlinks=False):
                        st = entry.stat(follow_symlinks=False)
                        digest.update(os.fsencode(entry.path) + b"\0")
                        digest.update(f"{st.st_size}:{st.st_mtime_ns}:{st.st_ctime_ns}\n".encode())
                        if metadata["cache_check"] == "content":
                            with open(entry.path, "rb") as source:
                                digest.update(hashlib.file_digest(source, "sha256").digest())
    return digest.hexdigest()


def baseline_counts(args, binary, game, baseline, parents, settings, metadata):
    def calculate():
        return Counter(report_identity(r) for r in scan(binary, game, baseline, parents, **settings))

    if not args.cache_dir:
        return calculate(), "disabled", None
    cache = args.cache_dir.resolve()
    roots = [game / "game", game / "jomini", game / "clausewitz", *parents, baseline]
    if any(cache.is_relative_to(r) for r in [*roots, Path(metadata["mod"])]):
        raise ValueError("Choose a baseline cache outside the input trees")
    cache.mkdir(parents=True, exist_ok=True)
    # Candidate state belongs in the run report; only baseline inputs key this cache.
    key_fields = {k: v for k, v in metadata.items() if k not in {"mod", "input_revisions"}}
    key_fields["cache_format"] = CACHE_FORMAT
    key = baseline_key(key_fields, roots)
    destination = cache / f"{key}.json"
    snapshot = (key_fields, roots, key)
    if destination.is_file() and not args.refresh_baseline:
        data = json.loads(destination.read_text(encoding="utf-8"))
        if not isinstance(data, dict) or data.get("key") != key or not isinstance(data.get("counts"), dict):
            raise ValueError(f"Invalid baseline cache {destination}; use --refresh-baseline")
        if any(type(n) is not int or n <= 0 for n in data["counts"].values()):
            raise ValueError(f"Invalid counts in {destination}; use --refresh-baseline")
        return Counter(data["counts"]), "hit", snapshot
    counts = calculate()
    if baseline_key(key_fields, roots) != key:
        raise ValueError("Baseline inputs changed during validation; retry the comparison")
    out = tempfile.NamedTemporaryFile("w", dir=cache, delete=False, encoding="utf-8")
    pending = Path(out.name)
    try:
        with out:
            json.dump({"key": key, "counts": counts}, out)
        os.replace(pending, destination)
    finally:
        pending.unlink(missing_ok=True)
    return counts, "miss", snapshot


def validate(args):
    started = time.monotonic()
    game, mod = directory(args.game), directory(args.mod)
    parents = [directory(p) for p in args.parent]
    baseline = directory(args.baseline) if args.baseline else None
    if len(set(parents)) != len(parents) or mod in parents or baseline in parents:
        raise ValueError("A parent must occur once and be distinct from each primary mod")
    if not (game / "game/events/witch_events.txt").is_file():
        raise ValueError(f"Expected the CK3 installation root containing game/events: {game}")
    binary = args.tiger.resolve(strict=True)
    info = subprocess.run([binary, "build-info"], capture_output=True, encoding="utf-8", check=True)
    build = dict(line.split("=", 1) for line in info.stdout.splitlines())
    if build.get("validator") != "ck3-tiger" or not build.get("build") or not build.get("rules"):
        raise ValueError("Expected a CK3 port build with build-info")
    version = json.loads((game / "launcher/launcher-settings.json").read_text(encoding="utf-8-sig"))["rawVersion"]
    settings = dict(stable=bool(baseline or args.deterministic),
                    show_parents=bool(baseline or args.show_parents), show_vanilla=args.show_vanilla)
    with binary.open("rb") as exe:
        sha = hashlib.file_digest(exe, "sha256").hexdigest()
    metadata = dict(build=build, binary_sha256=sha, game=str(game), game_version=version,
                    mod=str(mod), parents=[str(p) for p in parents], baseline=str(baseline) if baseline else None,
                    language="english", pod=True, cache_check=args.cache_check, **settings)
    inputs = [("Clausewitz", game / "clausewitz"), ("Jomini", game / "jomini"), ("CK3", game / "game")]
    inputs.extend((f"parent-{i}", parent) for i, parent in enumerate(parents))
    inputs.append(("MOD", mod))
    if baseline:
        inputs.append(("baseline", baseline))
    metadata["input_revisions"] = [input_revision(label, path.resolve()) for label, path in inputs]
    if args.lookup is not None:
        found = lookup(binary, game, mod, parents, args.lookup, **settings)
        metadata["seconds"] = round(time.monotonic() - started, 3)
        return {"metadata": metadata, "lookup": found}
    previous, cache_status, snapshot = (baseline_counts(args, binary, game, baseline, parents, settings, metadata)
                                        if baseline else (Counter(), "disabled", None))
    reports = []
    total = 0
    for row in scan(binary, game, mod, parents, **settings):
        total += 1
        if baseline:
            key = report_identity(row)
            if previous[key]:
                previous[key] -= 1
                continue
        reports.append(row)
    if snapshot is not None:
        fields, roots, expected = snapshot
        if baseline_key(fields, roots) != expected:
            raise ValueError("Baseline inputs changed during the comparison; retry")
    metadata.update(cache=cache_status, candidate_reports=total, removed=sum(previous.values()),
                    seconds=round(time.monotonic() - started, 3))
    return {"metadata": metadata, "reports": reports}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game", required=True, help="CK3 install root containing game/, jomini/ and clausewitz/")
    parser.add_argument("--mod", required=True, help="Primary mod directory or bare drop-in tree")
    parser.add_argument("--parent", action="append", default=[], help="Loaded parent directory, lowest priority first")
    parser.add_argument("--tiger", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--lookup", metavar="SYMBOL", help="Show loaded definitions and known callers of an identifier")
    parser.add_argument("--baseline", help="Unmodified primary tree for comparison, including affected parent reports")
    parser.add_argument("--cache-dir", type=Path, help="Reuse a matching baseline from this directory")
    parser.add_argument("--cache-check", choices=("metadata", "content"), default="metadata",
                        help="Also hash all input bytes in content mode (for timestamp-preserving rewrites)")
    parser.add_argument("--refresh-baseline", action="store_true", help="Recalculate and replace the matching cache entry")
    parser.add_argument("--deterministic", action="store_true", help="Repeatable single-thread run (automatic with --baseline)")
    parser.add_argument("--show-parents", action="store_true")
    parser.add_argument("--show-vanilla", action="store_true")
    parser.add_argument("--fail-on", choices=SEVERITIES, default="warning")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if (args.cache_dir and not args.baseline) or (args.refresh_baseline and not args.cache_dir):
        parser.error("--cache-dir needs --baseline; --refresh-baseline needs --cache-dir")
    if args.lookup is not None and (not args.lookup.strip() or args.baseline):
        parser.error("--lookup needs a nonempty symbol and a single load order")
    try:
        result = validate(args)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        print(f"Validation unavailable: {exc}", file=sys.stderr)
        return 2
    if args.json:
        json.dump(result, sys.stdout, ensure_ascii=True)
        print()
    else:
        for entry in result["metadata"]["input_revisions"]:
            state = {True: "dirty", False: "clean", None: "status unavailable"}[entry["dirty"]]
            print(f'{entry["label"]} {entry["path"]}  {entry["revision"] or "revision unavailable"} ({state})')
        if args.lookup is not None:
            print_lookup(result["lookup"])
        for row in result.get("reports", []):
            for index, loc in enumerate(row["locations"] or [{}]):
                where = f"{loc.get('from', '?')} {loc.get('path', '?')}:{loc.get('linenr') or 0}"
                print(f"{row['severity']}({row['key']}) {where}: {row['message']}" if index == 0
                      else f"  via {where}")
        if "reports" in result:
            meta = result["metadata"]
            print(f"{len(result['reports'])} findings, {meta['removed']} removed; {meta['seconds']} s; baseline cache {meta['cache']}")
    if "lookup" in result:
        return int(not any(row["status"] in ("effective", "contribution")
                           for row in result["lookup"]["definitions"]))
    return int(any(SEVERITIES.index(row["severity"]) >= SEVERITIES.index(args.fail_on)
                   for row in result["reports"]))


if __name__ == "__main__":
    sys.exit(main())
