"""Exercise lookup and revision provenance through the public runner and the release binary."""

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run


def script(root, relative, content):
    path = root / relative
    path.parent.mkdir(exist_ok=True, parents=True)
    path.write_text(content, encoding="utf-8-sig", newline="\r\n")


def git(root, *args):
    return subprocess.check_output(["git", "-C", root, *args], encoding="utf-8").strip()


class LookupTests(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        root = Path(folder.name)
        self.game = root / "CK3"
        shutil.copytree(run.ROOT / "tests/files/ck3", self.game)
        (self.game / "launcher").mkdir()
        (self.game / "launcher/launcher-settings.json").write_text('{"rawVersion":"1.21.0.3"}')
        script(self.game, "game/events/witch_events.txt", "")
        self.parent = root / "parent"
        self.parent.mkdir()
        self.repo = root / "integration"
        self.mod = self.repo / "devpod"
        self.mod.mkdir(parents=True)
        self.command = [sys.executable, "-B", str(run.ROOT / "workspace/run.py"), "--game",
                        str(self.game), "--parent", str(self.parent), "--mod", str(self.mod)]

    def lookup(self, symbol, expected=0):
        result = subprocess.run([*self.command, "--json", "--lookup", symbol],
                                capture_output=True, encoding="utf-8")
        self.assertEqual(result.returncode, expected, result.stderr or result.stdout[:5000])
        return json.loads(result.stdout)

    def test_load_order_and_semantic_callers(self):
        key = "tiger_provenance_effect"
        same = "common/scripted_effects/50_shared.txt"
        older = "common/scripted_effects/01_original.txt"
        masked = "common/scripted_effects/replaced/hidden.txt"
        callers = "common/on_action/tiger_lookup_test.txt"
        for root in (self.parent, self.mod):
            (root / "descriptor.mod").write_text(
                f'name="fixture"\npath="{root.as_posix()}"\n' +
                ('replace_path="common/scripted_effects/replaced"\n' if root == self.mod else ""))
        script(self.parent, same, f"{key} = {{ add_gold = $HIDDEN$ }}\n"
               f"tiger_masked_caller = {{ {key} = {{ HIDDEN = 1 }} }}\n")
        script(self.parent, older, f"{key} = {{ add_gold = $FORMER$ }}\n")
        script(self.parent, masked, f"{key} = {{ add_gold = $REPLACED$ }}\n"
               "tiger_only_hidden = { add_gold = 3 }\n")
        script(self.mod, same, f"{key} = {{ add_gold = $GOLD$ }}\n"
               "tiger_dispatch_effect = { tiger_$TARGET$_effect = { GOLD = 2 } }\n")
        script(self.parent, callers,
               "three_year_playable_pulse = {\n"
               f"  # {key} = {{ GOLD = 12 }}\n"
               "  effect = {\n"
               f"    {key} = {{ GOLD = 1 }}\n"
               "    tiger_dispatch_effect = { TARGET = provenance }\n"
               f"    set_variable = {{ name = {key} value = 1 }}\n"
               "  }\n}\n")

        data = self.lookup(key)["lookup"]
        definitions = {(d["from"], d["path"]): d for d in data["definitions"]}
        self.assertEqual(len(data["definitions"]), 4)
        for path, reason, param in [(older, "overridden_definition", "FORMER"),
                                    (same, "same_path", "HIDDEN"), (masked, "replace_path", "REPLACED")]:
            row = definitions["parent-0", path]
            self.assertEqual((row["status"], row["reason"], row["parameters"]),
                             ("shadowed", reason, [param]))
        winner = definitions["MOD", same]
        self.assertEqual((winner["status"], winner["parameters"], winner["line"]), ("effective", ["GOLD"], 1))
        sites = {(s["from"], s["path"], s["line"]): s for s in data["callers"]}
        self.assertEqual(set(sites), {("parent-0", callers, 4), ("MOD", same, 2)})
        self.assertEqual(sites["parent-0", callers, 4]["caller"], "three_year_playable_pulse")
        expanded = sites["MOD", same, 2]
        self.assertEqual(expanded["caller"], "tiger_dispatch_effect")
        self.assertIn(("parent-0", callers, 5),
                      [(s["from"], s["path"], s["line"]) for s in expanded["expansion"]])
        hidden = self.lookup("tiger_only_hidden", 1)["lookup"]
        self.assertEqual([(r["status"], r["reason"]) for r in hidden["definitions"]],
                         [("shadowed", "replace_path")])
        missing = self.lookup("tiger_does_not_exist", 1)["lookup"]
        self.assertEqual((missing["definitions"], missing["callers"]), ([], []))
        plain = subprocess.check_output([*self.command, "--lookup", key], encoding="utf-8")
        self.assertIn("GOLD", plain)
        self.assertIn("three_year_playable_pulse", plain)

    def test_additive_on_actions_and_value(self):
        rel = "common/on_action/a.txt"
        script(self.parent, rel, "tiger_contributed = { events = { } }\n")
        script(self.mod, "common/on_action/b.txt", "tiger_contributed = { events = { } }\n")
        rows = self.lookup("tiger_contributed")["lookup"]["definitions"]
        self.assertEqual([(r["from"], r["status"]) for r in rows],
                         [("parent-0", "contribution"), ("MOD", "contribution")])
        script(self.parent, "common/script_values/values.txt", "tiger_lookup_cost = 3\n")
        script(self.mod, rel, "three_year_playable_pulse = { effect = { add_gold = tiger_lookup_cost } }\n")
        value = self.lookup("tiger_lookup_cost")["lookup"]
        self.assertEqual([(r["from"], r["line"]) for r in value["definitions"]], [("parent-0", 1)])
        self.assertEqual([(r["from"], r["path"], r["line"]) for r in value["callers"]], [("MOD", rel, 1)])

    def test_nested_git_revisions(self):
        target = "common/script_values/values.txt"
        script(self.mod, target, "tiger_cost = 1\n")
        git(self.repo, "init", "-q")
        git(self.repo, "add", "devpod")
        git(self.repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "commit", "-qm", "fixture")
        sha = git(self.repo, "rev-parse", "HEAD")
        (self.repo / "outside.txt").write_text("untracked sibling")

        def revisions():
            rows = self.lookup("tiger_cost")["metadata"]["input_revisions"]
            self.assertEqual([r["label"] for r in rows], ["Clausewitz", "Jomini", "CK3", "parent-0", "MOD"])
            self.assertIsNone(rows[-2]["revision"])
            self.assertIsNone(rows[-2]["dirty"])
            return rows[-1]

        entry = revisions()
        self.assertEqual((entry["path"], entry["repository"], entry["revision"], entry["dirty"]),
                         (str(self.mod.resolve()), str(self.repo.resolve()), sha, False))
        script(self.mod, "common/script_values/extra.txt", "tiger_extra = 2\n")
        self.assertTrue(revisions()["dirty"])
        (self.mod / "common/script_values/extra.txt").unlink()
        script(self.mod, target, "tiger_cost = 2\n")
        self.assertTrue(revisions()["dirty"])

    def test_resolved_item_and_trigger(self):
        script(self.parent, "common/traits/traits.txt", "tiger_lookup_trait = { }\n")
        script(self.parent, "common/scripted_triggers/triggers.txt",
               "tiger_can_fight = { has_trait = tiger_lookup_trait }\n")
        rel = "common/on_action/combat.txt"
        script(self.mod, rel, "three_year_playable_pulse = {\n"
               "  trigger = { tiger_can_fight = yes }\n"
               "  effect = { add_trait = tiger_lookup_trait }\n}\n")
        trait = self.lookup("tiger_lookup_trait")["lookup"]
        self.assertEqual([(s["kind"], s["caller"], s["line"]) for s in trait["callers"]],
                         [("trait", "three_year_playable_pulse", 3), ("trait", "tiger_can_fight", 1)])
        trigger = self.lookup("tiger_can_fight")["lookup"]
        self.assertEqual([(s["kind"], s["caller"], s["line"]) for s in trigger["callers"]],
                         [("scripted_trigger", "three_year_playable_pulse", 2)])

    def test_event_number_aliases_and_priority(self):
        hidden = "events/00_hidden.txt"
        script(self.parent, hidden, "namespace = tiger\ntiger.0001 = { type = character_event }\n")
        script(self.mod, hidden, "namespace = other\n")
        script(self.parent, "events/10_first.txt",
               "namespace = tiger\ntiger.01 = { type = character_event id_override_priority = 2 }\n")
        script(self.mod, "events/20_late.txt",
               "namespace = tiger\ntiger.0001 = { type = character_event id_override_priority = 1 }\n")
        calls = "common/on_action/events.txt"
        script(self.mod, calls, "three_year_playable_pulse = { events = { tiger.001 } }\n")
        for query in ("tiger.1", "tiger.0001"):
            with self.subTest(query=query):
                event = self.lookup(query)["lookup"]
                self.assertEqual({(r["path"], r["status"], r.get("reason")) for r in event["definitions"]},
                                 {(hidden, "shadowed", "same_path"), ("events/10_first.txt", "effective", None),
                                  ("events/20_late.txt", "shadowed", "overridden_definition")})
                self.assertEqual([(s["path"], s["caller"], s["target"]["path"]) for s in event["callers"]],
                                 [(calls, "three_year_playable_pulse", "events/10_first.txt")])


if __name__ == "__main__":
    unittest.main()
