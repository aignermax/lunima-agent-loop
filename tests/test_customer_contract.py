import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools" / "ux-tester"))
from customer_contract import assess, load_goals, policy_hash
from customer_workspace import fresh


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        (self.path / "shots").mkdir()
        (self.path / "shots" / "shot-001.png").write_bytes(b"test fixture")
        self.goals = [{"id": "first", "persona": "Jonas", "goal": "Run example", "success": "Result visible"}]
        self.identity = {"repo": "owner/repo", "pr": 12, "sha": "a" * 40, "policy": "policy"}
        self.data = {"identity": self.identity, "partial": False, "interrupted": False,
                     "findings": [], "scenarios": [{"scenario": "first", "verdict": "pass",
                     "screenshot": 1, "actions": 3, "observation": "Result is visible"}]}

    def verdict(self, data=None):
        return assess(data or self.data, self.goals, self.path, self.identity)[0]

    def test_complete_observed_goal_passes(self):
        self.assertEqual(self.verdict(), "passed")

    def test_incomplete_runs_never_pass(self):
        for field in ("partial", "interrupted"):
            with self.subTest(field=field):
                data = copy.deepcopy(self.data)
                data[field] = True
                self.assertEqual(self.verdict(data), "blocked")

    def test_identity_and_evidence_required(self):
        for change in (lambda d: d.update(identity={}), lambda d: d.update(scenarios=[]),
                       lambda d: d["scenarios"].append(d["scenarios"][0]),
                       lambda d: d["scenarios"][0].update(screenshot=99),
                       lambda d: d["scenarios"][0].update(actions=0),
                       lambda d: d["scenarios"][0].update(scenario="invented"),
                       lambda d: d["scenarios"][0].update(verdict="skipped")):
            data = copy.deepcopy(self.data)
            change(data)
            self.assertEqual(self.verdict(data), "blocked")

    def test_findings_and_failed_goals_block_acceptance(self):
        for severity, expected in (("minor", "passed"), ("major", "needs_changes"),
                                   ("critical", "needs_changes"), ("unknown", "blocked")):
            self.data["findings"] = [{"severity": severity}]
            self.assertEqual(self.verdict(), expected)
        self.data["findings"] = []
        self.data["scenarios"][0]["verdict"] = "fail"
        self.assertEqual(self.verdict(), "needs_changes")

    def test_scenario_file_rejects_empty_and_duplicates(self):
        path = self.path / "goals.json"
        for data in ([], self.goals * 2, [{"id": "empty"}]):
            path.write_text(json.dumps(data))
            with self.assertRaises(ValueError):
                load_goals(path)

    def test_cache_invalidates_new_commit_policy_model_age_and_clock_skew(self):
        record = {"identity": self.identity, "finished_at": 1000, "status": "passed"}
        self.assertTrue(fresh(record, self.identity, 1100, 200))
        self.assertFalse(fresh(record, dict(self.identity, sha="b" * 40), 1100, 200))
        self.assertFalse(fresh(record, dict(self.identity, policy="new"), 1100, 200))
        self.assertFalse(fresh(record, self.identity, 1200, 200))
        self.assertFalse(fresh(record, self.identity, 900, 200))
        self.assertNotEqual(policy_hash(self.goals, "model-a"), policy_hash(self.goals, "model-b"))


if __name__ == "__main__":
    unittest.main()
