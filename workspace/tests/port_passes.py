"""Check CK3 1.21 schemas and script APIs through the public workspace runner."""

import argparse
import json
from pathlib import Path
import sys

from first_pass import HERE, location, run


PASSES = {
    "districts": ("district", "building", "history"),
    "trade": ("trade", "trade_script"),
    "maps": ("maps",),
    "gui": ("gui",),
    "flags": ("flag_conversion",),
}


def compare_fixture(mod, reports):
    rules = json.loads((mod / "expected.json").read_text(encoding="utf-8-sig"))
    pending = []
    for rule in rules:
        lines = (mod / rule["path"]).read_text(encoding="utf-8-sig").splitlines()
        matches = [n for n, line in enumerate(lines, 1) if rule["marker"] in line]
        if len(matches) != 1:
            raise ValueError(f'{rule["path"]}: marker {rule["marker"]!r} occurs {len(matches)} times')
        pending.append((rule, matches[0]))
    problems = []
    for report in reports:
        source = report["locations"][0]
        for i, (rule, line) in enumerate(pending):
            if (location(source) == ("MOD", rule["path"]) and source["linenr"] == line
                    and report["key"] == rule["key"] and rule["contains"] in report["message"]
                    and report["severity"] == rule.get("severity", report["severity"])):
                pending.pop(i)
                break
        else:
            problems.append(f'unexpected {report["key"]} at {source["path"]}:{source["linenr"]}'
                            f' — {report["message"]}')
    for rule, line in pending:
        problems.append(f'missing {rule["key"]} at {rule["path"]}:{line} — {rule["contains"]}')
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=HERE.parents[2])
    parser.add_argument("--game", type=Path, required=True)
    parser.add_argument("--tiger", type=Path, required=True)
    parser.add_argument("--pass", dest="passes", action="append", choices=PASSES)
    args = parser.parse_args()
    failed = False
    for group in args.passes or PASSES:
        for case in PASSES[group]:
            mod = HERE / "port_mods" / case
            try:
                reports = run(args, mod, [args.workspace / "POD Reference"])
                problems = compare_fixture(mod, reports)
            except (OSError, ValueError, KeyError, RuntimeError, IndexError) as error:
                print(f"ERROR {case}: {error}", file=sys.stderr)
                return 2
            if problems:
                print(f"FAIL {case}", *problems, sep="\n  ", flush=True)
                failed = True
            else:
                print(f"PASS {case} ({len(reports)} expected warnings/errors)", flush=True)
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
