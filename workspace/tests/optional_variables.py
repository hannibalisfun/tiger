"""Verify optional Asakku depth reads against the loaded PoD parent using the shared runner."""

import argparse
from pathlib import Path
import tempfile

from first_pass import HERE, location, run
from test_lookup import script


VALUES = "common/script_values/tiger_asakku.txt"
EFFECTS = "common/scripted_effects/tiger_asakku.txt"
TRIGGERS = "common/scripted_triggers/tiger_asakku.txt"
HOOK = "common/on_action/tiger_asakku.txt"


def check(args, case):
    with tempfile.TemporaryDirectory(prefix="tiger-asakku-") as temp:
        mod = Path(temp)
        script(mod, VALUES, "tiger_depth_read = {\n\tvalue = var:asakku_depth\n}\n"
               "tiger_depth_alias = tiger_depth_read\n")
        script(mod, EFFECTS, "tiger_use_depth = { add_prestige = tiger_depth_alias }\n"
               "tiger_depth_wrapper = { tiger_use_depth = yes }\n")
        script(mod, TRIGGERS, "tiger_positive_depth = { tiger_depth_read > 0 }\n")
        lines = [
            "on_death = { on_actions = { tiger_asakku_test } }",
            "tiger_asakku_test = {",
            "\teffect = {",
            "\t\tadd_prestige = POD_asakku_depth",
            "\t\tif = {",
            "\t\t\tlimit = { has_variable = asakku_depth }",
            "\t\t\ttiger_depth_wrapper = yes",
            "\t\t\ttiger_use_depth = yes",
            "\t\t}",
            "\t\tif = {",
            "\t\t\tlimit = { trigger_if = { limit = { has_variable = asakku_depth } tiger_positive_depth = yes } }",
            "\t\t\tadd_prestige = 1",
            "\t\t}",
        ]
        if case == "unguarded":
            lines.append("\t\ttiger_depth_wrapper = yes")
        script(mod, HOOK, "\n".join([*lines, "\t}", "}", ""]))
        reports = run(args, mod, [args.workspace / "POD Reference"])
        if case == "guarded":
            assert reports == [], reports
        else:
            assert len(reports) == 1, reports
            report = reports[0]
            assert (report["key"], report["severity"]) == ("optional-variable", "warning"), report
            assert "var:asakku_depth" in report["message"], report
            assert [(*location(p), p["linenr"]) for p in report["locations"]] == [
                ("MOD", HOOK, len(lines)), ("MOD", VALUES, 2),
            ], report
        print(f"PASS {case}: {len(reports)} warnings/errors", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=HERE.parents[2])
    parser.add_argument("--game", type=Path, required=True)
    parser.add_argument("--tiger", type=Path, required=True)
    parser.add_argument("--case", choices=("unguarded", "guarded"))
    args = parser.parse_args()
    for case in ([args.case] if args.case else ["unguarded", "guarded"]):
        check(args, case)


if __name__ == "__main__":
    main()
