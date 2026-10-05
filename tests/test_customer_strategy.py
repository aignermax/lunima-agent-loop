import base64
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools" / "ux-tester"))
from customer_strategy import strategy_snapshot


class StrategyTests(unittest.TestCase):
    def responses(self, suffix=""):
        def source(text):
            return json.dumps({"sha": "blob" + suffix, "content": base64.b64encode(text.encode()).decode()})
        return [source("## Vision\nNAND2TETRIS for photonics\n## Rungs\ndeveloper notes"),
                source("## Persona 4: Jonas\nStudent new to photonics\n## How agents should use\ninternal instructions"),
                json.dumps({"body": "old single-chip limit\n## North star (added later)\nmulti-chiplet manufacturable computer",
                            "updatedAt": "2026-08-15" + suffix, "url": "https://github.com/o/r/issues/537"})]

    def test_pinned_sources_personas_and_latest_north_star(self):
        with patch("customer_strategy.command", side_effect=self.responses()) as command:
            result = strategy_snapshot("o/r", "exact-commit")
        self.assertIn("?ref=exact-commit", command.call_args_list[0].args[0][-1])
        for expected in ("NAND2TETRIS", "Student new to photonics", "multi-chiplet", "not foundry sign-off"):
            self.assertIn(expected, result["brief"])
        for excluded in ("old single-chip limit", "developer notes", "internal instructions"):
            self.assertNotIn(excluded, result["brief"])
        with patch("customer_strategy.command", side_effect=self.responses("changed")):
            self.assertNotEqual(result["fingerprint"], strategy_snapshot("o/r", "exact-commit")["fingerprint"])

    def test_missing_strategy_fails_closed(self):
        with patch("customer_strategy.command", side_effect=RuntimeError("GitHub unavailable")):
            with self.assertRaises(RuntimeError):
                strategy_snapshot("o/r", "sha")
        responses = self.responses()
        responses[-1] = json.dumps({"body": "old strategy only"})
        with patch("customer_strategy.command", side_effect=responses):
            with self.assertRaisesRegex(ValueError, "North star"):
                strategy_snapshot("o/r", "sha")
