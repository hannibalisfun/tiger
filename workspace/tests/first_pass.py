"""Exercise the shared runner on the CK3 1.21 fixture and real PoD load orders."""

import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys


HERE = Path(__file__).resolve().parent
RUNNER = HERE.parent / "run.py"
FIXTURE = HERE / "first_pass_mod"
DECISIONS = "common/decisions/tiger_district_mage.txt"
SIBLINGS = ("Splat-Contracts-PoD", "Patron-Offices-PoD", "PoD-Silver-Pack-Landless")


def run(args, mod, parents):
    command = [
        sys.executable, "-B", str(RUNNER), "--game", str(args.game), "--mod", str(mod),
        "--tiger", str(args.tiger), "--json",
    ]
    for parent in parents:
        command.extend(["--parent", str(parent)])
    result = subprocess.run(command, capture_output=True, encoding="utf-8")
    if result.returncode not in (0, 1):
        raise RuntimeError(f"runner exit {result.returncode}: {result.stderr or result.stdout}")
    report = json.loads(result.stdout)
    if not isinstance(report["reports"], list) or not isinstance(report["metadata"], dict):
        raise ValueError("incomplete JSON report from shared runner")
    findings = [r for r in report["reports"] if r["severity"] in {"fatal", "error", "warning"}]
    if result.returncode != int(bool(findings)):
        raise ValueError(f"runner exit {result.returncode} for {len(findings)} warnings/errors")
    return findings


def location(entry):
    return (entry["from"], entry["path"].replace("\\", "/"))


def compare(actual, expected):
    problems = []
    for label, diff in (("unexpected", actual - expected), ("missing", expected - actual)):
        problems.extend(f"{label} {count} x {key}" for key, count in diff.items())
    return problems


def camp_findings(reports, standalone):
    actual = Counter(
        (r["severity"], r["key"], r["message"], tuple(map(location, r["locations"])))
        for r in reports
    )
    expected = Counter()
    if standalone:
        garrison_source = (("MOD", "common/script_values/podcamp_raid_values.txt"),)
        for tier in range(1, 7):
            message = (f"domicile parameter podcamp_warden_garrison_{tier} "
                       "not defined in common/domiciles/buildings/")
            expected["error", "missing-item", message, garrison_source] = 1
        message = "trait flag silverpack_caern_access not defined in common/traits/"
        flag_source = (("MOD", "common/scripted_triggers/podcamp_raid_support_triggers.txt"),)
        expected["error", "missing-item", message, flag_source] = 1
    return compare(actual, expected)


def fixture_findings(reports):
    rules = {
        "interrupted trigger chain": ("error", "if-else", ("`trigger_else_if`", "`trigger_else`")),
        "unterminated trigger chain": ("error", "if-else", ("`trigger_else_if`", "`trigger_else`")),
        "province receiver": ("warning", "scopes", ("province", "district")),
        "character argument": ("warning", "scopes", ("character", "building")),
        "unknown trait": ("error", "unknown-field", ("tiger_assess_nonexistent_trait",)),
    }
    # The marker follows the rejected construct. Moving a fixture line keeps its assertion.
    markers = {}
    for number, line in enumerate((FIXTURE / DECISIONS).read_text(encoding="utf-8-sig").splitlines(), 1):
        if "# expect: " in line:
            markers[number] = line.partition("# expect: ")[2].strip()
    if Counter(markers.values()) != Counter(rules.keys()):
        raise ValueError("fixture and expected diagnostic markers disagree")
    found = Counter()
    for report in reports:
        source = report["locations"][0]
        marker = markers.get(source["linenr"]) if location(source) == ("MOD", DECISIONS) else None
        if marker:
            severity, key, terms = rules[marker]
            if (report["severity"] == severity and report["key"] == key
                    and all(term in report["message"].lower() for term in terms)):
                found[marker] += 1
                continue
        found[f'{report["key"]} at {source["path"]}:{source["linenr"]}: {report["message"]}'] += 1
    return compare(found, Counter(rules.keys()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=HERE.parents[2], help="PoD development workspace")
    parser.add_argument("--game", type=Path, required=True, help="CK3 installation root")
    parser.add_argument("--tiger", type=Path, required=True, help="CK3 1.21 port executable")
    args = parser.parse_args()
    pod = args.workspace / "POD Reference"
    camp = args.workspace / "Camp-Pressure-PoD"
    cases = (
        ("District and Mage fixture", FIXTURE, [pod], fixture_findings),
        ("Camp Pressure with PoD", camp, [pod], lambda reports: camp_findings(reports, True)),
        ("Camp Pressure with PoD and siblings", camp, [pod, *(args.workspace / s for s in SIBLINGS)],
         lambda reports: camp_findings(reports, False)),
    )
    failed = False
    for label, mod, parents, check in cases:
        try:
            reports = run(args, mod, parents)
            problems = check(reports)
        except (OSError, ValueError, KeyError, IndexError, RuntimeError) as error:
            print(f"ERROR {label}: {error}", file=sys.stderr)
            return 2
        if problems:
            failed = True
            print(f"FAIL {label}", *problems, sep="\n  ")
        else:
            print(f"PASS {label} ({len(reports)} expected warnings/errors)")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
