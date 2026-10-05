"""Drive the real model/tool/report loop with scripted API and desktop adapters."""
import argparse
import importlib.util
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1] / "tools" / "ux-tester"
sys.path.insert(0, str(ROOT))
from customer_contract import assess


class Block:
    def __init__(self, name=None, inputs=None, toolset=None, text=""):
        self.name, self.input, self.toolset_name, self.text = name, inputs, toolset, text
        self.type, self.id = ("tool_use" if name else "text"), "call-test"

    def model_dump(self, **kwargs):
        return {"type": self.type, "name": self.name, "input": self.input, "text": self.text}


def response(blocks, stop="tool_use"):
    return argparse.Namespace(content=blocks, stop_reason=stop,
        usage=argparse.Namespace(input_tokens=10, output_tokens=10, cache_read_input_tokens=0))


class DriverTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("customer_test_driver", ROOT / "ux_tester.py")
        cls.driver = importlib.util.module_from_spec(spec)
        native = {name: Mock() for name in ("anthropic", "mss", "pyautogui", "pygetwindow", "PIL")}
        native["pyautogui"].FailSafeException = type("FailSafeException", (Exception,), {})
        native["anthropic"].BadRequestError = type("BadRequestError", (Exception,), {})
        sys.modules[spec.name] = cls.driver
        with patch.dict(sys.modules, native), patch("ctypes.windll", Mock(), create=True):
            spec.loader.exec_module(cls.driver)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.goals = [{"id": "demo", "persona": "Jonas", "goal": "Run", "success": "See output"}]
        self.identity = {"repo": "o/r", "pr": 1, "sha": "a" * 40, "policy": "test"}
        (self.path / "goals.json").write_text(json.dumps(self.goals))
        (self.path / "request.json").write_text(json.dumps(self.identity))
        self.args = argparse.Namespace(exact_report_dir=str(self.path / "evidence"), report_dir="unused",
            customer_goals=str(self.path / "goals.json"), customer_identity=str(self.path / "request.json"),
            app="fake.exe", attach=False, window_title="Lunima", max_steps=10, max_turns=3,
            keep_open=False, file_issues=False)

    def drive(self, responses):
        class Screen:
            def __init__(self, folder, window):
                self.folder, self.count = folder, 0
                self.width, self.height, self.scale = 800, 600, 1

            def grab(self, region=None):
                self.count += 1
                (self.folder / f"shot-{self.count:03d}.png").write_bytes(b"test screenshot")
                return "fakeimage", self.count

        client = Mock()
        client.beta.messages.create.side_effect = responses
        app = argparse.Namespace(proc=None, window=Mock(), hwnd=1)
        with patch.multiple(self.driver, make_dpi_aware=Mock(), require_unlocked_desktop=Mock(),
                            launch_app=Mock(return_value=app), Screen=Screen, validate_action=Mock(),
                            execute_action=Mock(return_value="OK"), measure_hang=Mock(return_value=0)), \
             patch.object(self.driver.anthropic, "Anthropic", return_value=client), \
             patch.object(self.driver.time, "sleep"):
            self.driver.run(self.args)
        data = json.loads((self.path / "evidence" / "findings.json").read_text())
        return data, client

    def test_customer_click_to_evidence_to_acceptance(self):
        data, _ = self.drive([
            response([Block("left_click", {"coordinate": [40, 40]}, "computer")]),
            response([Block("note_scenario", {"scenario": "demo", "verdict": "pass",
                "observation": "Output visible after clicking run", "screenshot": 2})]),
            response([Block(text="Goal completed")], "end_turn")])
        self.assertEqual(assess(data, self.goals, self.path / "evidence", self.identity)[0], "passed")
        self.assertEqual(data["scenarios"][0]["actions"], 1)
        self.assertTrue((self.path / "evidence" / "report.md").exists())

    def test_strategy_and_persona_brief_reaches_customer(self):
        self.goals[0]["customer_context"] = "NAND2TETRIS for photonics; Jonas is a student new to photonics"
        (self.path / "goals.json").write_text(json.dumps(self.goals))
        _, client = self.drive([response([], "end_turn")])
        prompt = str(client.beta.messages.create.call_args.kwargs)
        self.assertIn("Jonas is a student", prompt)
        self.assertIn("NAND2TETRIS for photonics", prompt)

    def test_refusal_is_blocked(self):
        data, _ = self.drive([response([], "refusal")])
        self.assertEqual(assess(data, self.goals, self.path / "evidence", self.identity)[0], "blocked")

    def test_claiming_success_without_interaction_is_blocked(self):
        data, _ = self.drive([response([Block("note_scenario", {"scenario": "demo", "verdict": "pass",
            "observation": "Looks good", "screenshot": 1})]), response([], "end_turn")])
        self.assertEqual(assess(data, self.goals, self.path / "evidence", self.identity)[0], "blocked")

    def test_non_computer_tool_loop_still_hits_turn_budget(self):
        data, client = self.drive([response([Block("unknown", {})])] * 3)
        self.assertEqual(client.beta.messages.create.call_count, 3)
        self.assertTrue(data["interrupted"])


if __name__ == "__main__":
    unittest.main()
