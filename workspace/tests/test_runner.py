"""Runner regression tests. Build ck3-tiger in release mode before running this file."""

import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run


class RunnerTests(unittest.TestCase):
    def test_json_boundaries(self):
        row = {"severity": "error", "confidence": "strong", "key": "if-else",
               "message": "x" * 80000, "locations": []}
        self.assertEqual(list(run.json_rows(io.StringIO(json.dumps([row, row])))), [row, row])
        for text in ("", "{}", "[", "[{},]", "[]{}", "[null]", '[{"severity":"typo"}]'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                list(run.json_rows(io.StringIO(text)))

    def test_baseline(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            game = root / "CK3"
            shutil.copytree(run.ROOT / "tests/files/ck3", game)
            (game / "launcher").mkdir()
            (game / "launcher/launcher-settings.json").write_text('{"rawVersion":"1.21.0.3"}')
            (game / "game/events/witch_events.txt").write_text("", encoding="utf-8-sig")
            baseline = root / "original"
            mod = root / "edited"
            parent = root / "parent"
            parent.mkdir()
            baseline.mkdir()
            mod.mkdir()
            cache = root / "cache"
            command = [sys.executable, "-B", str(run.ROOT / "workspace/run.py"), "--game", str(game),
                       "--parent", str(parent), "--mod", str(mod), "--baseline", str(baseline),
                       "--cache-dir", str(cache), "--json"]

            def scan():
                result = subprocess.run(command, capture_output=True, encoding="utf-8")
                self.assertIn(result.returncode, (0, 1), result.stderr)
                return result.returncode, json.loads(result.stdout)

            code, first = scan()
            self.assertEqual((code, first["metadata"]["cache"], first["reports"]), (0, "miss", []))
            relative = Path("common/on_action/test-trigger-chains.txt")
            (mod / relative).parent.mkdir(parents=True)
            shutil.copyfile(run.ROOT / "tests/files/mod1" / relative, mod / relative)
            code, edited = scan()
            self.assertEqual((code, edited["metadata"]["cache"]), (1, "hit"))
            self.assertEqual(sum(r["key"] == "if-else" for r in edited["reports"]), 2)
            (baseline / relative).parent.mkdir(parents=True)
            shutil.copyfile(mod / relative, baseline / relative)
            code, updated = scan()
            self.assertEqual((code, updated["metadata"]["cache"], updated["reports"]), (0, "miss", []))
            (parent / "descriptor.mod").write_text('name="Parent"')
            self.assertEqual(scan()[1]["metadata"]["cache"], "miss")
            (game / "game/events/witch_events.txt").unlink()
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 2)


if __name__ == "__main__":
    unittest.main()
