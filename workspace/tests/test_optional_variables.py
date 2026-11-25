"""Regression cases for caller-specific presence of a scoped CK3 variable."""

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from test_lookup import run, script


class OptionalVariableTests(unittest.TestCase):
    def test_scopes_guards_and_writes(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            game, mod = root / "ck3", root / "mod"
            shutil.copytree(run.ROOT / "tests/files/ck3", game)
            (game / "launcher").mkdir()
            (game / "launcher/launcher-settings.json").write_text('{"rawVersion":"1.21.0.3"}')
            script(game, "game/events/witch_events.txt", "")
            values = "common/script_values/depth.txt"
            effects = "common/scripted_effects/depth.txt"
            hooks = "common/on_action/depth.txt"
            script(mod, values,
                   "tiger_default = { if = { limit = { has_variable = tiger_other } value = var:tiger_other } else = { value = 0 } }\n"
                   "tiger_other_read = var:tiger_other\n"
                   "tiger_read = var:tiger_depth\n"
                   "POD_asakku_depth = var:asakku_depth\n")
            script(mod, effects, "tiger_reader = { add_prestige = tiger_read }\n"
                   "tiger_clear = { remove_variable = tiger_depth }\n"
                   "tiger_nested_clear = { tiger_clear = yes }\n"
                   "tiger_macro = { add_prestige = var:$NAME$ }\n")
            body = """on_death = { effect = {
    if = {
        limit = { has_variable = tiger_depth }
        tiger_reader = yes
        tiger_reader = yes
        primary_spouse = { tiger_reader = yes } # wrong object
        tiger_macro = { NAME = tiger_depth }
    }
    tiger_reader = yes # outside guard
    if = { limit = { OR = { has_variable = tiger_depth always = yes } } tiger_reader = yes } # disjunction
    if = { limit = { NOT = { has_variable = tiger_depth } } tiger_reader = yes } # negation
    if = { limit = { exists = var:tiger_depth } tiger_reader = yes }
    if = { limit = { exists = primary_spouse.var:tiger_depth } add_prestige = 1 }
    if = { limit = { always = no } set_variable = { name = tiger_depth value = 1 } }
    tiger_reader = yes # conditional write
    hidden_effect = { set_variable = { name = tiger_depth value = 1 } }
    tiger_reader = yes
    tiger_nested_clear = yes
    tiger_reader = yes # removed in nested call
    tiger_macro = { NAME = tiger_depth } # expanded raw read
    add_prestige = tiger_other_read
    add_prestige = POD_asakku_depth # replaced accessor
} }
"""
            script(mod, hooks, body)
            cmd = [sys.executable, "-B", str(run.ROOT / "workspace/run.py"), "--game", str(game),
                   "--mod", str(mod), "--optional-variable", "tiger_depth", "--json"]
            scan = subprocess.run(cmd, capture_output=True, encoding="utf-8")
            self.assertEqual(scan.returncode, 1, scan.stderr)
            output = json.loads(scan.stdout)
            self.assertEqual(output["metadata"]["optional_variables"], ["asakku_depth", "tiger_depth"])
            findings = [r for r in output["reports"] if r["key"] == "optional-variable"]
            expected = [n for n, line in enumerate(body.splitlines(), 1) if "#" in line]
            actual = []
            for report in findings:
                call, read, *_ = report["locations"]
                self.assertEqual(call["path"].replace("\\", "/"), hooks, report)
                self.assertIn(read["path"].replace("\\", "/"), [values, effects], report)
                actual.append(call["linenr"])
            self.assertEqual(sorted(actual), expected)


if __name__ == "__main__":
    unittest.main()
