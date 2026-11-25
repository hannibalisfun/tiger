"""Verify active and fully masked Git edits against the real combat effect and its seven callers."""

import argparse
from collections import Counter
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from first_pass import HERE, RUNNER
from test_lookup import git, run


EFFECT = "pod_combat_select_next_actor_effect"
SOURCE = Path("common/scripted_effects/POD_combat/pod_combat_group_effects.txt")
CALLERS = {
    "POD_combat_initialize_and_start_duel_effect", "pod_combat_advance_one_ai_turn_effect",
    "pod_combat_run_remaining_ai_turns_effect", "pod_combat_run_ai_until_human_turn_effect",
    "pod_combat_settle_step_after_human_turn_effect", "pod_combat_settle_step_before_human_turn_effect",
    "pod_combat_advance_to_next_round_effect",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=HERE.parents[2])
    parser.add_argument("--game", type=Path, required=True)
    parser.add_argument("--tiger", type=Path, required=True)
    args = parser.parse_args()
    combat = args.workspace / "POD-Combat-Integration/devpod"
    with tempfile.TemporaryDirectory(prefix="ck3-impact-combat-") as temp:
        root = Path(temp)
        edited, winner = root / "edited", root / "winner"
        for mod in (edited, winner):
            (mod / SOURCE).parent.mkdir(parents=True)
            shutil.copyfile(combat / SOURCE, mod / SOURCE)
        git(edited, "init", "-q")
        git(edited, "add", ".")
        git(edited, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "commit", "-qm", "original combat effect")
        original = (edited / SOURCE).read_bytes()
        anchor = b"set_variable = { name = pod_combat_best_init value = -999 }"
        if original.count(anchor) != 1:
            raise ValueError("Combat initiative seed moved; inspect the current effect")
        (edited / SOURCE).write_bytes(original.replace(anchor, anchor.replace(b"-999", b"-1000")))
        base = [sys.executable, "-B", str(RUNNER), "--json", "--deterministic", "--game", str(args.game),
                "--tiger", str(args.tiger), "--parent", str(combat)]

        def validate(*arguments):
            process = subprocess.run([*base, *arguments], capture_output=True, encoding="utf-8")
            if process.returncode not in (0, 1):
                raise RuntimeError(process.stderr)
            payload = json.loads(process.stdout)
            assert process.returncode == int(any(r["severity"] in ("warning", "error", "fatal")
                                                 for r in payload["reports"]))
            return payload

        ordinary = validate("--mod", str(winner))["reports"]
        expected = Counter(run.report_identity(r) for r in ordinary)
        for label, arguments in [
            ("active", ["--mod", str(edited)]),
            ("shadowed", ["--parent", str(edited), "--mod", str(winner), "--impact-layer", "parent-1"]),
        ]:
            payload = validate("--impact", "HEAD", *arguments)
            assert Counter(run.report_identity(r) for r in payload["reports"]) == expected
            info = payload["impact"]
            assert info["base"] == git(edited, "rev-parse", "HEAD")
            assert info["skipped"] == [], info["skipped"]
            row, = info["symbols"]
            assert (row["symbol"], row["kind"]) == (EFFECT, "scripted_effect")
            if label == "active":
                assert row["status"] == "changed"
                assert len(row["callers"]) == 7 and {c["caller"] for c in row["callers"]} == CALLERS
                assert all(c["target"]["from"] == "MOD" for c in row["callers"])
            else:
                assert row["status"] == "unchanged" and row["callers"] == []
                assert row["changes"][0]["file_mask"] == {"from": "MOD", "reason": "same_path"}
            print(f'PASS {label} combat edit ({len(row["callers"])} direct callers, '
                  f'{len(expected)} diagnostic groups matched ordinary validation)', flush=True)


if __name__ == "__main__":
    main()
