"""Check competing on-action blocks and an additive child through the shared CK3 runner."""

import argparse
from collections import Counter
from pathlib import Path
import tempfile

from first_pass import HERE, compare, location, run
from test_lookup import script


EARLY = "common/on_action/20_tiger_death.txt"
PARENT = "common/on_action/40_tiger_death.txt"
LATE = "common/on_action/60_tiger_death.txt"
HOOK = "tiger_on_death"


def conflict_case(args, mod, parents):
    script(mod, EARLY,
           f"{HOOK} = {{\n\teffect = {{ add_prestige = 1 }}\n}}\n"
           f"{HOOK} = {{\n\teffect = {{ add_prestige = 2 }}\n}}\n")
    script(mod, LATE, f"{HOOK} = {{\n\ttrigger = {{ always = no }}\n\ttrigger = {{ always = yes }}\n}}\n")
    reports = run(args, mod, parents)
    actual = Counter((row["severity"], row["key"], row["message"],
                      tuple((*location(p), p["linenr"], p["tag"]) for p in row["locations"]))
                     for row in reports)

    def conflict(field, ignored, winner):
        return ("warning", "on-action-conflict", f"on-action `{HOOK}` has competing `{field}` blocks",
                ((*ignored, "ignored block"), (*winner, "winning block")))

    return compare(actual, Counter([
        conflict("effect", ("MOD", EARLY, 2), ("parent-1", PARENT, 4)),
        conflict("effect", ("MOD", EARLY, 5), ("parent-1", PARENT, 4)),
        conflict("trigger", ("parent-1", PARENT, 3), ("MOD", LATE, 2)),
        conflict("trigger", ("MOD", LATE, 3), ("MOD", LATE, 2)),
    ]))


def child_case(args, mod, parents):
    script(mod, LATE,
           f"{HOOK} = {{ on_actions = {{ tiger_death_child }} }}\n"
           "tiger_death_child = {\n\ttrigger = { always = yes }\n\teffect = { add_prestige = 2 }\n}\n")
    # This parent file is fully masked. Its blocks must never enter conflict resolution.
    masked = "common/on_action/30_tiger_masked.txt"
    script(parents[-1], masked, "tiger_death_child = { effect = { } trigger = { always = no } }\n")
    script(mod, masked, "# Replaced by the primary mod.\n")
    return [f'unexpected {row["key"]}: {row["message"]}' for row in run(args, mod, parents)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=HERE.parents[2])
    parser.add_argument("--game", type=Path, required=True)
    parser.add_argument("--tiger", type=Path, required=True)
    parser.add_argument("--case", choices=("conflict", "child"))
    args = parser.parse_args()
    failed = False
    for name, check in (("conflict", conflict_case), ("child", child_case)):
        if args.case and args.case != name:
            continue
        with tempfile.TemporaryDirectory(prefix="tiger-on-action-") as folder:
            parent, mod = Path(folder) / "parent", Path(folder) / "mod"
            script(parent, PARENT, f"on_death = {{ on_actions = {{ {HOOK} }} }}\n"
                   f"{HOOK} = {{\n\ttrigger = {{ always = yes }}\n\teffect = {{ add_prestige = 3 }}\n}}\n")
            problems = check(args, mod, [args.workspace / "POD Reference", parent])
        print(f"{'FAIL' if problems else 'PASS'} on-action {name}", *problems, sep="\n  ", flush=True)
        failed |= bool(problems)
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
