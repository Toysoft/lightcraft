"""Checks for the generated release history; no network required."""

import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("release-history.py")
spec = importlib.util.spec_from_file_location("release_history", SCRIPT)
history = importlib.util.module_from_spec(spec)
spec.loader.exec_module(history)


def release(tag, date, **extra):
    return {"tag_name": tag, "published_at": date, "draft": False, "prerelease": False, **extra}


class ReleaseHistoryTests(unittest.TestCase):
    def test_uses_publication_dates_orders_releases_and_omits_unpublished_entries(self):
        text = history.render([
            release("v0.1.0", "2026-10-02T22:31:34Z", created_at="2026-10-01T00:00:00Z"),
            release("v0.3.0", None, draft=True),
            release("v0.2.0-rc.1", None, prerelease=True),
            release("v0.2.0", "2026-10-05T06:49:58Z"),
        ])
        self.assertIn("- v0.2.0 — 2026-10-05\n- v0.1.0 — 2026-10-02", text)
        self.assertNotIn("rc.1", text)
        self.assertNotIn("v0.3.0", text)
        self.assertNotIn("2026-10-01", text)

    def test_rejects_invalid_dates_tags_and_duplicate_releases(self):
        for data in [
            [release("v1.0.0", "2026-02-30T00:00:00Z")],
            [release("v1.0.0\n### injected", "2026-10-02T00:00:00Z")],
            [release("v1.0.0", "2026-10-02T00:00:00Z")] * 2,
            {"message": "rate limited"},
        ]:
            with self.subTest(data=data), self.assertRaises(ValueError):
                history.render(data)

    def test_empty_history_does_not_invent_a_release(self):
        self.assertIn("No published releases yet.", history.render([]))

    def test_fetches_every_page(self):
        pages = [json.dumps([release("v1.0.0", "2026-10-02T00:00:00Z")] * 100), "[]"]
        with patch.object(history, "urlopen", side_effect=[io.StringIO(page) for page in pages]) as fetch:
            self.assertEqual(len(history.fetch_releases()), 100)
            self.assertEqual(fetch.call_count, 2)
            self.assertTrue(fetch.call_args.args[0].full_url.endswith("page=2"))

    def test_cli_keeps_the_previous_snapshot_when_input_is_invalid(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "releases.json"
            output = Path(directory) / "history.md"
            source.write_text(json.dumps([release("v1.0.0", "not a date")]))
            output.write_text("previous snapshot")
            run = subprocess.run([sys.executable, str(SCRIPT), "--input", str(source), "--output", str(output)], capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertEqual(output.read_text(), "previous snapshot")
            self.assertEqual(sorted(path.name for path in Path(directory).iterdir()), ["history.md", "releases.json"])


if __name__ == "__main__":
    unittest.main()
