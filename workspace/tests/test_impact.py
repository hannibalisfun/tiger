"""Public runner regressions for Git diff impact and full validation."""

import contextlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from test_lookup import git, run, script
import impact_git


class ImpactTests(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        root = Path(folder.name)
        self.game = root / "game-install"
        shutil.copytree(run.ROOT / "tests/files/ck3", self.game)
        (self.game / "launcher").mkdir()
        (self.game / "launcher/launcher-settings.json").write_text('{"rawVersion":"1.21.0.3"}')
        script(self.game, "game/events/witch_events.txt", "")
        self.repo = root / "integration"
        self.mod = self.repo / "devpod"
        self.mod.mkdir(parents=True)
        self.parent = root / "parent"
        self.parent.mkdir()
        git(self.repo, "init", "-q")
        self.args = [str(run.ROOT / "workspace/run.py"), "--mod", str(self.mod),
                     "--parent", str(self.parent), "--game", str(self.game), "--deterministic", "--json"]

    def commit(self):
        git(self.repo, "add", ".")
        git(self.repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "commit", "--allow-empty", "-qm", "baseline")

    def validate(self, *args):
        # The count is taken at the real executable boundary; build-info reads metadata only.
        execute = subprocess.run
        validations = []

        def record(command, *positional, **kwargs):
            if Path(command[0]) == run.DEFAULT_BINARY and "build-info" not in command:
                validations.append(command)
            return execute(command, *positional, **kwargs)

        output = io.StringIO()
        with patch.object(sys, "argv", [*self.args, *args]), patch.object(subprocess, "run", record), \
                contextlib.redirect_stdout(output):
            code = run.main()
        self.assertIn(code, (0, 1))
        self.assertEqual(len(validations), 1)
        return code, json.loads(output.getvalue())

    def test_batch_uses_worktree_and_keeps_all_diagnostics(self):
        rel = "common/scripted_effects/tiger impact.txt"
        val = "common/script_values/tiger.txt"
        calls = "common/on_action/callers.txt"
        original = ("tiger_a = { add_gold = 1 }\n"
                    "tiger_macro = { add_gold = $GOLD$ }\n"
                    "tiger_unmodified = { add_gold = 4 }\n")
        script(self.mod, rel, original)
        script(self.mod, val, "tiger_size = 1\n")
        script(self.mod, calls, "three_year_playable_pulse = { effect = {\n"
               " tiger_a = yes\n tiger_macro = { GOLD = tiger_size }\n"
               " add_gold = tiger_size\n tiger_nonexistent_effect = yes\n} }\n")
        self.commit()
        script(self.mod, rel, original.replace("gold = 1", "gold = 3").replace("$GOLD$", "2"))
        git(self.repo, "add", "devpod")
        script(self.mod, rel, "# shifted line\n" + original.replace("gold = 1", "gold = 2")
               .replace("add_gold = $GOLD$", "add_gold = { value = $GOLD$ add = 3 }"))
        script(self.mod, val, "tiger_size = 2\n")
        (self.mod / "notes.md").write_text("out of scope")
        (self.repo / "outside.txt").write_text("outside mod")
        normal_code, normal = self.validate()
        code, changed = self.validate("--impact", "HEAD")
        self.assertEqual(code, 1)
        self.assertEqual((code, changed["reports"]), (normal_code, normal["reports"]))
        payload = changed["impact"]
        self.assertEqual(payload["base"], git(self.repo, "rev-parse", "HEAD"))
        self.assertEqual([row["path"] for row in payload["skipped"]], ["notes.md"])
        found = {row["symbol"]: row for row in payload["symbols"]}
        self.assertEqual(set(found), {"tiger_a", "tiger_macro", "tiger_size"})
        for name in found:
            self.assertEqual(found[name]["status"], "changed")
            self.assertTrue(found[name]["callers"])
        self.assertEqual(found["tiger_a"]["callers"][0]["caller"], "three_year_playable_pulse")

    def test_shadowed_by_another_definition_and_same_name_other_kind(self):
        loser = "common/scripted_effects/10_original.txt"
        script(self.mod, loser, "tiger_shared = { add_gold = 1 }\n")
        script(self.mod, "common/scripted_effects/90_winner.txt", "tiger_shared = { add_gold = 2 }\n")
        script(self.mod, "common/scripted_triggers/shared.txt", "tiger_shared = { always = yes }\n")
        script(self.mod, "common/on_action/calls.txt", "three_year_playable_pulse = {\n"
               "trigger = { tiger_shared = yes }\neffect = { tiger_shared = yes }\n}\n")
        self.commit()
        script(self.mod, loser, "tiger_shared = { add_gold = 3 }\n")
        row, = self.validate("--impact")[1]["impact"]["symbols"]
        self.assertEqual((row["kind"], row["status"], row["callers"]), ("scripted_effect", "unchanged", []))

    def test_file_local_names_bind_to_the_edited_definition(self):
        template = ("namespace = NAMESPACE\nscripted_effect tiger_local = { add_gold = 1 }\n"
                    "NAMESPACE.0001 = { type = character_event immediate = { tiger_local = yes } }\n")
        for name in ("red", "blue"):
            script(self.mod, f"events/{name}.txt", template.replace("NAMESPACE", name))
        self.commit()
        script(self.mod, "events/red.txt", template.replace("NAMESPACE", "red").replace("gold = 1", "gold = 2"))
        row, = self.validate("--impact")[1]["impact"]["symbols"]
        self.assertEqual((row["symbol"], row["status"]), ("tiger_local", "changed"))
        self.assertEqual([(c["caller"], c["target"]["path"]) for c in row["callers"]],
                         [("red.0001", "events/red.txt")])
        script(self.mod, "events/red.txt", template.replace("NAMESPACE", "red")
               .replace("scripted_effect tiger_local = { add_gold = 1 }\n", ""))
        row, = self.validate("--impact")[1]["impact"]["symbols"]
        self.assertEqual((row["status"], row["callers"]), ("removed", []))
        self.assertEqual([c["caller"] for c in row["unresolved_callers"]], ["red.0001"])

    def test_add_and_remove_mask_different_symbols(self):
        rel = "common/script_values/shared.txt"
        script(self.parent, rel, "tiger_lower = 1\n")
        script(self.mod, "common/on_action/calls.txt",
               "three_year_playable_pulse = { effect = { add_gold = tiger_lower } }\n")
        self.commit()
        script(self.mod, rel, "tiger_override = 2\n")  # untracked overlay
        result = self.validate("--impact")[1]["impact"]
        self.assertEqual({r["symbol"]: r["status"] for r in result["symbols"]},
                         {"tiger_lower": "removed", "tiger_override": "changed"})
        self.commit()
        (self.mod / rel).unlink()
        found = {r["symbol"]: r for r in self.validate("--impact")[1]["impact"]["symbols"]}
        self.assertEqual(found["tiger_override"]["status"], "removed")
        self.assertEqual(found["tiger_lower"]["status"], "changed")
        self.assertEqual(found["tiger_lower"]["changes"][0]["after_from"], "parent-0")
        self.assertEqual(len(found["tiger_lower"]["callers"]), 1)

    def test_empty_and_comment_only_diff(self):
        rel = "common/scripted_effects/comments.txt"
        script(self.mod, rel, "tiger_unchanged = { add_gold = 1 }\n")
        self.commit()
        self.assertEqual(self.validate("--impact")[1]["impact"]["symbols"], [])
        script(self.mod, rel, "# ignored\ntiger_unchanged = {\n add_gold = 1 # still the same\n}\n")
        result = self.validate("--impact")[1]["impact"]
        self.assertEqual((result["symbols"], result["skipped"]), ([], []))

    def test_deleted_references_and_historical_parse_errors(self):
        effect = "common/scripted_effects/deleted.txt"
        trigger = "common/scripted_triggers/deleted.txt"
        broken = "common/script_values/broken.txt"
        calls = "common/on_action/calls.txt"
        script(self.mod, effect, "tiger_missing_effect = { add_gold = 1 }\n")
        script(self.mod, trigger, "tiger_missing_trigger = { always = yes }\n")
        script(self.mod, broken, "tiger_broken = {\n")
        script(self.mod, calls, "three_year_playable_pulse = {\n trigger = { tiger_missing_trigger = yes }\n"
               " effect = { tiger_missing_effect = yes }\n}\n")
        self.commit()
        for rel in (effect, trigger, broken):
            (self.mod / rel).unlink()
        code, result = self.validate("--impact")
        self.assertEqual(code, 1)
        rows = result["impact"]["symbols"]
        self.assertEqual({r["symbol"] for r in rows}, {"tiger_missing_effect", "tiger_missing_trigger"})
        for row in rows:
            self.assertEqual((row["status"], row["callers"]), ("removed", []))
            self.assertEqual([(r["path"], r["caller"]) for r in row["unresolved_callers"]],
                             [(calls, "three_year_playable_pulse")])
        self.assertEqual([r["path"] for r in result["impact"]["skipped"]], [broken])
        self.assertFalse(any(r["key"] == "brace-error" for r in result["reports"]))

    def test_git_inventory_detects_edits_during_validation(self):
        rel = "common/script_values/moved value.txt"
        script(self.mod, rel, "tiger_cost = 1\n")
        self.commit()
        moved = "common/script_values/renamed value.txt"
        (self.mod / rel).rename(self.mod / moved)
        (self.mod / ".gitignore").write_text("ignored.txt\n")
        (self.mod / "ignored.txt").write_text("ignored")
        with tempfile.TemporaryDirectory() as temp:
            inventory = impact_git.collect(self.mod, "HEAD", Path(temp))
            sides = {r["path"]: (r["before"] is not None, r["after"] is not None) for r in inventory["files"]}
            self.assertEqual(sides, {rel: (True, False), moved: (False, True)})
            self.assertEqual([r["path"] for r in inventory["skipped"]], [".gitignore"])
            impact_git.verify(self.mod, inventory)
            script(self.mod, moved, "tiger_cost = 2\n")
            with self.assertRaisesRegex(ValueError, "changed"):
                impact_git.verify(self.mod, inventory)


if __name__ == "__main__":
    unittest.main()
