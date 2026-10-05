import argparse
import json
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools" / "ux-tester"))
import customer_cycle as cycle
from customer_workspace import cycle_lock, prepare, targets
from customer_desktop import app_environment, validate_action


class CycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        strategy = patch.object(cycle, "strategy_snapshot", return_value={
            "fingerprint": "strategy-v1", "brief": "NAND2TETRIS for photonics; Jonas learns gates", "sources": {}})
        self.strategy = strategy.start()
        self.addCleanup(strategy.stop)
        self.args = argparse.Namespace(state_dir=self.path, feedback=self.path / "feedback.md", repo="owner/repo",
            base="dev-ki", label="agent-pr", scenarios=cycle.ROOT / "customer_scenarios.json", model="test-model",
            max_reviews=1, max_age=86400, max_steps=10, max_turns=10, timeout=1, project="App/App.csproj")
        self.targets = [{"key": "pr-1", "pr": 1, "sha": "a" * 40, "ref": "refs/pull/1/head"},
                        {"key": "baseline", "pr": None, "sha": "b" * 40, "ref": "refs/heads/dev-ki"}]
        (self.path / "goals").mkdir()
        (self.path / "goals" / "pr-1.json").write_text(json.dumps({"sha": "a" * 40, "goals": [
            {"id": "pr-export", "persona": "Peter", "goal": "Export changed design", "success": "Saved output visible"}]}))

    def fake_review(self, args, target, identity, path):
        return {"identity": identity, "status": "needs_changes", "reason": "Observed confusion",
                "finished_at": time.time()}

    def test_baseline_budget_then_pr_and_cache(self):
        with patch.object(cycle, "targets", return_value=self.targets), patch.object(cycle, "review", side_effect=self.fake_review) as review:
            self.assertEqual(cycle.cycle(self.args), 0)
            self.assertEqual(review.call_count, 1)
            data = json.loads(self.args.feedback.with_suffix(".json").read_text())
            self.assertEqual([r["status"] for r in data["reviews"]], ["needs_changes", "pending"])
            cycle.cycle(self.args)
            self.assertEqual(review.call_count, 2)
            cycle.cycle(self.args)
            self.assertEqual(review.call_count, 2)

    def test_push_during_review_is_not_accepted(self):
        changed = [dict(self.targets[0], sha="c" * 40), self.targets[1]]
        self.args.max_reviews = 2
        with patch.object(cycle, "targets", side_effect=[self.targets, changed]), patch.object(cycle, "review", side_effect=self.fake_review):
            cycle.cycle(self.args)
        data = json.loads(self.args.feedback.with_suffix(".json").read_text())
        self.assertEqual(data["reviews"][1]["status"], "blocked")

    def test_blocked_retries_cannot_starve_later_prs(self):
        for number in (2, 3):
            self.targets.append(dict(self.targets[0], key=f"pr-{number}", pr=number))
            (self.path / "goals" / f"pr-{number}.json").write_text(
                (self.path / "goals" / "pr-1.json").read_text())
        visited = []
        def blocked(args, target, identity, path):
            visited.append(target["key"])
            return {"identity": identity, "status": "blocked", "finished_at": time.time()}
        with patch.object(cycle, "targets", return_value=self.targets), patch.object(cycle, "review", side_effect=blocked):
            for now in (10000, 14000, 18000, 22000):
                with patch.object(cycle.time, "time", return_value=now):
                    cycle.cycle(self.args)
        self.assertEqual(visited, ["baseline", "pr-1", "pr-2", "pr-3"])

    def test_strategy_refresh_invalidates_cache_and_midrun_changes(self):
        with patch.object(cycle, "targets", return_value=self.targets), patch.object(cycle, "review", side_effect=self.fake_review) as review:
            cycle.cycle(self.args)
            first = json.loads((self.path / "reviews.json").read_text())["baseline"]["identity"]
            self.strategy.return_value = dict(self.strategy.return_value, fingerprint="strategy-v2")
            self.args.max_reviews = 2
            cycle.cycle(self.args)
            second = json.loads((self.path / "reviews.json").read_text())["baseline"]["identity"]
            self.assertNotEqual(first["policy"], second["policy"])
            self.assertEqual(review.call_count, 3)
            self.strategy.side_effect = [self.strategy.return_value, dict(self.strategy.return_value, fingerprint="v3")]
            cycle.cycle(self.args)
            data = json.loads(self.args.feedback.with_suffix(".json").read_text())
            self.assertTrue(all(r["status"] == "blocked" for r in data["reviews"]))

    def test_locked_desktop_does_not_build_or_call_model(self):
        with patch.object(cycle, "require_unlocked_desktop", side_effect=RuntimeError("Desktop locked")), patch.object(cycle, "prepare") as build:
            record = cycle.review(self.args, self.targets[0], {}, self.path)
        self.assertEqual(record["status"], "blocked")
        self.assertIn("locked", record["reason"])
        build.assert_not_called()

    def test_missing_or_stale_pr_goals_never_approve_generic_smoke(self):
        (self.path / "goals" / "pr-1.json").unlink()
        with patch.object(cycle, "targets", return_value=self.targets), patch.object(cycle, "review", side_effect=self.fake_review) as review:
            cycle.cycle(self.args)
            self.assertEqual(review.call_count, 1)
        data = json.loads(self.args.feedback.with_suffix(".json").read_text())
        self.assertEqual(data["reviews"][1]["status"], "blocked")
        self.assertIn("author", data["reviews"][1]["reason"])
        (self.path / "goals" / "pr-1.json").write_text('{"sha":"old","goals":[]}')
        with self.assertRaisesRegex(ValueError, "stale"):
            cycle.scenarios_for(self.args, self.targets[0])

    def test_deleted_evidence_invalidates_cached_pass(self):
        identity = {"sha": "a" * 40}
        record = {"identity": identity, "finished_at": time.time(), "status": "passed",
                  "report_dir": str(self.path / "deleted")}
        self.assertFalse(cycle.reusable(record, identity, self.args))

    def test_fetch_race_prevents_build(self):
        with patch("customer_workspace.command", side_effect=["", "", "", "wrong-sha"]):
            with self.assertRaisesRegex(RuntimeError, "advanced"):
                prepare("owner/repo", self.targets[0], self.path, "App/App.csproj")

    def test_cross_repo_drafts_and_unrelated_prs_are_not_executed(self):
        base = {"number": 1, "title": "Agent: test", "headRefOid": "a" * 40,
                "isDraft": False, "isCrossRepository": False, "labels": []}
        prs = [base, dict(base, number=2, isDraft=True), dict(base, number=3, isCrossRepository=True),
               dict(base, number=4, title="Unrelated")]
        with patch("customer_workspace.gh_json", return_value=prs), patch("customer_workspace.command", return_value='{"sha":"bbb"}'):
            result = targets("owner/repo", "dev-ki", "agent-pr")
        self.assertEqual([r["pr"] for r in result], [1, None])

    def test_app_profile_drops_runner_credentials(self):
        with patch.dict("os.environ", {"ANTHROPIC_API_KEY": "fake", "GH_TOKEN": "fake"}):
            env = app_environment(self.path / "profile")
        self.assertNotIn("GH_TOKEN", env)
        self.assertNotIn("ANTHROPIC_API_KEY", env)
        self.assertTrue(Path(env["APPDATA"]).is_relative_to(self.path))

    def test_shortcuts_and_outside_coordinates_rejected(self):
        window = argparse.Namespace(_hWnd=1, left=100, right=200, top=100, bottom=200)
        screen = argparse.Namespace(to_screen=lambda xy: xy)
        with patch("customer_desktop.require_target_foreground"):
            for key in ("Super+r", "Win-r", "Alt+Tab", "Ctrl+Shift+Escape", "Win + r", "Alt + Tab", "Ctrl + Shift + Escape"):
                with self.assertRaises(ValueError):
                    validate_action("key", {"text": key}, screen, window)
            with self.assertRaises(ValueError):
                validate_action("left_click", {"coordinate": [0, 0]}, screen, window)
            validate_action("left_click", {"coordinate": [150, 150]}, screen, window)
            for action in ("left_click", "right_click", "double_click", "scroll", "left_click_drag"):
                with self.assertRaises(ValueError):
                    validate_action(action, {}, screen, window)
            with self.assertRaises(ValueError):
                validate_action("left_click", {"coordinate": [150, 150], "text": " Win "}, screen, window)

    def test_lock_excludes_another_cycle_and_releases_after_exception(self):
        with self.assertRaisesRegex(ValueError, "test"):
            with cycle_lock(self.path / "lock"):
                with self.assertRaises(OSError):
                    with cycle_lock(self.path / "lock"):
                        pass
                raise ValueError("test")
        with cycle_lock(self.path / "lock"):
            pass


if __name__ == "__main__":
    unittest.main()
