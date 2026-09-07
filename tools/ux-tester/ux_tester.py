"""
Lunima UX tester — Claude drives the *real* desktop app via computer use.

What it does
------------
1. Launches Lunima (``dotnet CAP.Avalonia.dll``) on this machine's desktop, brings the window
   to the front and maximizes it.
2. Runs an agent loop with Claude and the ``computer_toolset_20260801`` tool: Claude sees
   screenshots, clicks, types and scrolls like a user walking the release checklist.
3. After every action the tester measures whether the app window is hung (Win32
   ``IsHungAppWindow``) and how long it stays unresponsive; that measurement is fed back to
   Claude as text, so "the UI hangs" becomes a number, not a feeling.
4. Claude records UX defects through a structured ``report_finding`` tool (placement,
   overlap, performance, help-text, physics, bug) with the screenshot that shows them.
5. Output: ``<report-dir>/report.md`` + ``findings.json`` + all screenshots. With
   ``--file-issues`` every finding of severity >= the threshold becomes a GitHub issue
   (label ``ux-finding``), screenshots committed to a ``ux-findings/<stamp>`` branch so the
   images render in the issue.

Requirements: Python 3.11+, ``pip install anthropic pyautogui mss pygetwindow pillow``,
an unlocked interactive Windows session, ``ANTHROPIC_API_KEY``, and for ``--file-issues``
the ``gh`` CLI (``GH_TOKEN``) plus a Lunima clone.

Usage (from the lunima-agent-loop root)::

    py -3 tools/ux-tester/ux_tester.py --app "C:/Users/.../CAP.Avalonia/bin/Debug/net10.0/CAP.Avalonia.dll" ^
        --checklist "C:/Users/.../Lunima-dev-ki/docs/RELEASE-CHECKLIST.md" --max-steps 120

The scenario list defaults to the *manual* items of the release checklist plus the
maintainer's standing UX concerns (see ``STANDING_CONCERNS``).
"""
from __future__ import annotations

import argparse
import base64
import ctypes
import io
import json
import math
import os
import re
import subprocess
import sys
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path

import anthropic
import mss
import pyautogui
import pygetwindow as gw
from PIL import Image

# --------------------------------------------------------------------------------------
# Configuration
# --------------------------------------------------------------------------------------

MODEL = os.environ.get("UX_TESTER_MODEL", "claude-fable-5-1")
EFFORT = os.environ.get("UX_TESTER_EFFORT", "high")
MAX_TOKENS = 16000
WINDOW_TITLE_HINT = "Lunima"
# Claude's vision limits for computer_toolset_20260801
MAX_LONG_EDGE = 2576
MAX_PIXELS = 3_750_000
# Keep the transcript lean: only the most recent screenshots stay as images.
KEEP_LAST_IMAGES = 6
HANG_POLL_SEC = 0.1
HANG_MAX_WAIT_SEC = 12.0
SETTLE_SEC = 0.35

STANDING_CONCERNS = """\
Standing concerns from the maintainer (check these *everywhere*, not as separate steps):
- PLACEMENT: features stuffed into the right sidebar as collapsible sections instead of an own
  window / dialog / flyout / canvas overlay. The right panel is for properties of the current
  selection only. Note every sidebar section that is *not* selection properties.
- OVERLAP / PHYSICS: photonic designs that make no sense — components or routes overlapping,
  routes crossing through components, dead or disconnected circuits, geometry that could not be
  fabricated. Use the bundled examples and anything the app generates (import, auto-route).
- PERFORMANCE: the UI hanging or reacting slowly. The tester tells you after each action whether
  the window was hung and for how long — treat > 0.3 s as a finding, > 1 s as major.
- HELP TEXT: (?) flyouts that are walls of text instead of <= 3 short sentences per section plus an
  illustrative animation. Open several (?) buttons and judge them.
- DISCOVERABILITY: features only reachable through nested expanders or hidden shortcuts.
"""

SYSTEM_PROMPT = """\
You are a senior UX designer and QA engineer testing Lunima, a desktop photonic-circuit design
tool (Avalonia/.NET on Windows). You control the real application through the computer tool.
The window is already open and maximized; work inside it (do not open other programs, do not
touch files outside the app, never close the app, never change system settings).

Method
- Work through the scenario list in order. For each scenario: perform the steps like a real user
  would, observe the result, compare with the expected result, and move on. Take a screenshot
  after each meaningful step; use `zoom` when you need to read small text.
- Be efficient: batch obviously-safe actions (click, type, key) in one turn, then screenshot.
- Judge like a UX designer, not a tester who only checks "does it exist": placement, clutter,
  clipped or overlapping elements, walls of text, missing feedback, sluggishness.
- Whenever you see a defect, call `report_finding` immediately with the screenshot index shown
  in the last tool result (`shot=N`). One finding per defect; do not repeat the same defect.
- The `note_scenario` tool records the verdict per scenario (pass / fail / blocked / skipped)
  with a one-line observation — call it once per scenario when you are done with it.
- If a step is impossible (dialog needs a file you do not have, feature missing), mark the
  scenario blocked with the reason and continue. Never invent results.
- If the app hangs for more than 12 s the tester will tell you; wait once more, then record a
  performance finding and continue if the UI recovers. If it stays hung, stop and say so.
- Stop when all scenarios are done or the step budget is exhausted. Finish with a short
  summary (3-6 bullets): the most important findings first.
"""

FINDING_TOOL = {
    "name": "report_finding",
    "description": "Record one UX/QA defect you observed in the app. Call once per distinct defect.",
    "strict": True,
    "input_schema": {
        "type": "object",
        "additionalProperties": False,
        "required": ["title", "severity", "category", "where", "steps", "expected", "observed", "screenshot", "suggestion"],
        "properties": {
            "title": {"type": "string", "description": "Short imperative title, <= 12 words."},
            "severity": {"type": "string", "enum": ["minor", "major", "critical"]},
            "category": {"type": "string", "enum": ["placement", "overlap", "performance", "help-text", "physics", "discoverability", "bug", "i18n", "other"]},
            "where": {"type": "string", "description": "Screen/panel/dialog where it happens."},
            "steps": {"type": "string", "description": "Click-by-click steps to reproduce, numbered."},
            "expected": {"type": "string"},
            "observed": {"type": "string"},
            "screenshot": {"type": "integer", "description": "Index (shot=N) of the screenshot that shows it; 0 if none."},
            "suggestion": {"type": "string", "description": "What a UX designer would do instead (surface, layout, animation, async...)."},
        },
    },
}

SCENARIO_TOOL = {
    "name": "note_scenario",
    "description": "Record the verdict for one scenario from the list once you are done with it.",
    "strict": True,
    "input_schema": {
        "type": "object",
        "additionalProperties": False,
        "required": ["scenario", "verdict", "observation"],
        "properties": {
            "scenario": {"type": "string", "description": "The scenario text (or its number)."},
            "verdict": {"type": "string", "enum": ["pass", "fail", "blocked", "skipped"]},
            "observation": {"type": "string", "description": "One line: what you saw."},
        },
    },
}

# --------------------------------------------------------------------------------------
# Win32 helpers
# --------------------------------------------------------------------------------------

user32 = ctypes.windll.user32


def make_dpi_aware() -> None:
    """Per-monitor DPI awareness so mss (physical pixels) and pyautogui agree on coordinates."""
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        try:
            user32.SetProcessDPIAware()
        except Exception:
            pass


def is_hung(hwnd: int) -> bool:
    return bool(user32.IsHungAppWindow(ctypes.c_void_p(hwnd)))


def measure_hang(hwnd: int) -> float:
    """Return how long (s) the window stays hung after an action, up to HANG_MAX_WAIT_SEC."""
    start = time.perf_counter()
    while time.perf_counter() - start < HANG_MAX_WAIT_SEC:
        if not is_hung(hwnd):
            return time.perf_counter() - start
        time.sleep(HANG_POLL_SEC)
    return HANG_MAX_WAIT_SEC


# --------------------------------------------------------------------------------------
# App lifecycle
# --------------------------------------------------------------------------------------

@dataclass
class AppHandle:
    proc: subprocess.Popen | None
    window: gw.Win32Window
    hwnd: int


def find_window(timeout: float = 60.0, title_hint: str = WINDOW_TITLE_HINT) -> gw.Win32Window:
    deadline = time.time() + timeout
    while time.time() < deadline:
        for w in gw.getWindowsWithTitle(title_hint):
            if w.width > 200 and w.height > 200:
                return w
        time.sleep(0.5)
    raise RuntimeError(f"No window with title containing '{title_hint}' appeared within {timeout}s")


def launch_app(app_path: str | None, attach: bool, title_hint: str) -> AppHandle:
    """Start the app (an .exe directly, a .dll via `dotnet`) unless attaching to a running window."""
    proc = None
    if not attach:
        if not app_path:
            raise SystemExit("--app <path to CAP.Desktop.exe or .dll> is required unless --attach is given")
        workdir = str(Path(app_path).parent)
        cmd = [app_path] if app_path.lower().endswith(".exe") else ["dotnet", app_path]
        proc = subprocess.Popen(cmd, cwd=workdir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    win = find_window(title_hint=title_hint)
    try:
        win.activate()
    except Exception:
        pass
    time.sleep(0.5)
    try:
        win.maximize()
    except Exception:
        pass
    time.sleep(1.5)
    return AppHandle(proc=proc, window=win, hwnd=win._hWnd)


# --------------------------------------------------------------------------------------
# Screen capture and action execution
# --------------------------------------------------------------------------------------

class Screen:
    def __init__(self, shots_dir: Path):
        self.shots_dir = shots_dir
        self.count = 0
        with mss.MSS() as sct:
            mon = sct.monitors[1]
        self.width, self.height = mon["width"], mon["height"]
        self.scale = self._scale_factor(self.width, self.height)

    @staticmethod
    def _scale_factor(width: int, height: int) -> float:
        long_edge_scale = MAX_LONG_EDGE / max(width, height)
        pixel_scale = math.sqrt(MAX_PIXELS / (width * height))
        return min(1.0, long_edge_scale, pixel_scale)

    def to_screen(self, xy) -> tuple[int, int]:
        return int(round(xy[0] / self.scale)), int(round(xy[1] / self.scale))

    def grab(self, region: list[int] | None = None) -> tuple[str, int]:
        """Capture (optionally a region in screenshot coordinates) → (base64 png, shot index)."""
        with mss.MSS() as sct:
            raw = sct.grab(sct.monitors[1])
        img = Image.frombytes("RGB", raw.size, raw.bgra, "raw", "BGRX")
        if self.scale < 1.0:
            img = img.resize((int(self.width * self.scale), int(self.height * self.scale)), Image.LANCZOS)
        if region:
            x0, y0, x1, y1 = region
            img = img.crop((max(0, x0), max(0, y0), min(img.width, x1), min(img.height, y1)))
            # zoomed crops may be upscaled for legibility, within the same limits
            factor = min(2.0, MAX_LONG_EDGE / max(img.width, img.height))
            if factor > 1.0:
                img = img.resize((int(img.width * factor), int(img.height * factor)), Image.LANCZOS)
        self.count += 1
        path = self.shots_dir / f"shot-{self.count:03d}.png"
        img.save(path, "PNG", optimize=True)
        buf = io.BytesIO()
        img.save(buf, "PNG", optimize=True)
        return base64.b64encode(buf.getvalue()).decode("ascii"), self.count


MODIFIER_MAP = {"ctrl": "ctrl", "control": "ctrl", "alt": "alt", "shift": "shift", "super": "win", "cmd": "win", "win": "win"}
KEY_MAP = {
    "return": "enter", "enter": "enter", "escape": "esc", "esc": "esc", "tab": "tab", "space": "space",
    "backspace": "backspace", "delete": "delete", "del": "delete", "home": "home", "end": "end",
    "pageup": "pageup", "page_up": "pageup", "pagedown": "pagedown", "page_down": "pagedown",
    "up": "up", "down": "down", "left": "left", "right": "right", "insert": "insert",
    "f1": "f1", "f2": "f2", "f3": "f3", "f4": "f4", "f5": "f5", "f6": "f6", "f7": "f7", "f8": "f8",
    "f9": "f9", "f10": "f10", "f11": "f11", "f12": "f12", "plus": "+", "minus": "-",
}


def parse_keys(text: str) -> list[str]:
    parts = [p.strip() for p in re.split(r"[+\-](?=.)", text) if p.strip()] if len(text) > 1 else [text]
    keys = []
    for p in parts:
        low = p.lower()
        keys.append(MODIFIER_MAP.get(low) or KEY_MAP.get(low) or (p if len(p) == 1 else low))
    return keys


def execute_action(name: str, inp: dict, screen: Screen) -> str:
    """Run one computer-toolset member on the real desktop. Returns a text result ('' for images)."""
    mods = [MODIFIER_MAP.get(m.strip().lower(), m.strip().lower()) for m in inp.get("text", "").split("+")] \
        if name.endswith("click") and inp.get("text") else []

    def with_mods(fn):
        for m in mods:
            pyautogui.keyDown(m)
        try:
            fn()
        finally:
            for m in reversed(mods):
                pyautogui.keyUp(m)

    if name in ("left_click", "right_click", "middle_click", "double_click", "triple_click"):
        if inp.get("coordinate"):
            x, y = screen.to_screen(inp["coordinate"])
            pyautogui.moveTo(x, y, duration=0.05)
        button = {"left_click": "left", "right_click": "right", "middle_click": "middle",
                  "double_click": "left", "triple_click": "left"}[name]
        clicks = {"double_click": 2, "triple_click": 3}.get(name, 1)
        with_mods(lambda: pyautogui.click(button=button, clicks=clicks, interval=0.08))
        return "OK"
    if name == "mouse_move":
        x, y = screen.to_screen(inp["coordinate"])
        pyautogui.moveTo(x, y, duration=0.05)
        return "OK"
    if name == "left_click_drag":
        x0, y0 = screen.to_screen(inp["start_coordinate"])
        x1, y1 = screen.to_screen(inp["coordinate"])
        pyautogui.moveTo(x0, y0, duration=0.05)
        pyautogui.mouseDown()
        pyautogui.moveTo(x1, y1, duration=0.4)
        pyautogui.mouseUp()
        return "OK"
    if name == "left_mouse_down":
        pyautogui.mouseDown(); return "OK"
    if name == "left_mouse_up":
        pyautogui.mouseUp(); return "OK"
    if name == "cursor_position":
        x, y = pyautogui.position()
        return f"X={int(x * screen.scale)}, Y={int(y * screen.scale)}"
    if name == "scroll":
        if inp.get("coordinate"):
            x, y = screen.to_screen(inp["coordinate"])
            pyautogui.moveTo(x, y, duration=0.05)
        amount = int(inp.get("scroll_amount", 3))
        direction = inp.get("scroll_direction", "down")
        if direction in ("up", "down"):
            pyautogui.scroll(amount * 120 if direction == "up" else -amount * 120)
        else:
            pyautogui.hscroll(amount * 120 if direction == "right" else -amount * 120)
        return "OK"
    if name == "type":
        pyautogui.write(inp["text"], interval=0.01)
        return "OK"
    if name == "key":
        keys = parse_keys(inp["text"])
        for _ in range(int(inp.get("repeat", 1))):
            if len(keys) > 1:
                pyautogui.hotkey(*keys)
            else:
                pyautogui.press(keys[0])
        return "OK"
    if name == "hold_key":
        keys = parse_keys(inp["text"])
        for k in keys:
            pyautogui.keyDown(k)
        time.sleep(min(float(inp.get("duration", 1)), 300))
        for k in reversed(keys):
            pyautogui.keyUp(k)
        return "OK"
    if name == "wait":
        time.sleep(min(float(inp.get("duration", 1)), 300))
        return "OK"
    raise ValueError(f"unsupported computer action: {name}")


# --------------------------------------------------------------------------------------
# Session state
# --------------------------------------------------------------------------------------

@dataclass
class Finding:
    title: str
    severity: str
    category: str
    where: str
    steps: str
    expected: str
    observed: str
    screenshot: int
    suggestion: str


@dataclass
class ScenarioNote:
    scenario: str
    verdict: str
    observation: str


@dataclass
class Session:
    findings: list[Finding] = field(default_factory=list)
    scenarios: list[ScenarioNote] = field(default_factory=list)
    hangs: list[tuple[int, float]] = field(default_factory=list)  # (step, seconds)
    steps: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read: int = 0
    summary: str = ""


def prune_images(messages: list[dict]) -> None:
    """Replace all but the last KEEP_LAST_IMAGES screenshot blocks with a text stub."""
    image_refs: list[dict] = []
    for m in messages:
        if m["role"] != "user" or not isinstance(m["content"], list):
            continue
        for block in m["content"]:
            if block.get("type") == "tool_result" and isinstance(block.get("content"), list):
                for c in block["content"]:
                    if c.get("type") == "image":
                        image_refs.append((block, c))
    stale = image_refs[:-KEEP_LAST_IMAGES] if len(image_refs) > KEEP_LAST_IMAGES else []
    for block, img in stale:
        block["content"] = [c for c in block["content"] if c is not img] + \
            [{"type": "text", "text": "[earlier screenshot removed to save context]"}]


# --------------------------------------------------------------------------------------
# Scenario loading
# --------------------------------------------------------------------------------------

def load_scenarios(checklist: str | None, extra: str | None, manual_only: bool, limit: int | None) -> list[str]:
    items: list[str] = []
    if checklist:
        section = ""
        for line in Path(checklist).read_text(encoding="utf-8").splitlines():
            if line.startswith("## "):
                section = line[3:].strip()
                continue
            m = re.match(r"^\s*- \[ \] (.+?)\s*(`\((auto|manual)[^`]*\)`)?\s*$", line)
            if not m:
                continue
            kind = m.group(3) or "manual"
            if manual_only and kind != "manual":
                continue
            items.append(f"[{section}] {m.group(1).strip()}")
    if extra:
        items.extend(l.strip() for l in Path(extra).read_text(encoding="utf-8").splitlines() if l.strip())
    if limit:
        items = items[:limit]
    return items


# --------------------------------------------------------------------------------------
# Agent loop
# --------------------------------------------------------------------------------------

def run(args: argparse.Namespace) -> Session:
    make_dpi_aware()
    pyautogui.FAILSAFE = True  # slam the mouse into the top-left corner to abort
    pyautogui.PAUSE = 0.05

    stamp = datetime.now().strftime("%Y-%m-%d_%H%M%S")
    report_dir = Path(args.report_dir) / stamp
    shots_dir = report_dir / "shots"
    shots_dir.mkdir(parents=True, exist_ok=True)

    scenarios = load_scenarios(args.checklist, args.scenarios, not args.include_auto, args.limit)
    if not scenarios:
        raise SystemExit("No scenarios — pass --checklist and/or --scenarios")

    app = launch_app(args.app, args.attach, args.window_title)
    screen = Screen(shots_dir)
    client = anthropic.Anthropic()
    session = Session()

    scenario_text = "\n".join(f"{i + 1}. {s}" for i, s in enumerate(scenarios))
    first_shot_b64, idx = screen.grab()
    messages: list[dict] = [{
        "role": "user",
        "content": [
            {"type": "text", "text": f"{STANDING_CONCERNS}\nScenarios to walk through ({len(scenarios)}):\n{scenario_text}\n\n"
                                     f"Screen is {int(screen.width * screen.scale)}x{int(screen.height * screen.scale)} in screenshot coordinates. "
                                     f"Step budget: {args.max_steps} actions. Here is the current screen (shot={idx}):"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": first_shot_b64}},
        ],
    }]

    tools = [{"type": "computer_toolset_20260801"}, FINDING_TOOL, SCENARIO_TOOL]
    log = (report_dir / "transcript.jsonl").open("a", encoding="utf-8")

    def call_model():
        kwargs = dict(model=MODEL, max_tokens=MAX_TOKENS, system=SYSTEM_PROMPT, tools=tools,
                      messages=messages, output_config={"effort": EFFORT},
                      cache_control={"type": "ephemeral"})
        # Refusal fallbacks are opt-in on Fable; if this account/endpoint rejects the beta,
        # fall back to the plain endpoint once and remember that.
        if not call_model.plain:
            try:
                return client.beta.messages.create(betas=["server-side-fallback-2026-07-01"], fallbacks="default", **kwargs)
            except anthropic.BadRequestError as e:
                print(f"[fallbacks beta rejected, using plain endpoint: {str(e)[:120]}]", flush=True)
                call_model.plain = True
        return client.messages.create(**kwargs)

    call_model.plain = False

    turns = 0
    while session.steps < args.max_steps:
        turns += 1
        prune_images(messages)
        resp = call_model()
        u = resp.usage
        session.input_tokens += u.input_tokens
        session.output_tokens += u.output_tokens
        session.cache_read += getattr(u, "cache_read_input_tokens", 0) or 0
        log.write(json.dumps({"turn": turns, "stop": resp.stop_reason, "content": [b.model_dump() for b in resp.content]}, default=str) + "\n")
        log.flush()

        if resp.stop_reason == "refusal":
            session.summary = "Model refused to continue (stop_reason=refusal)."
            break

        messages.append({"role": "assistant", "content": [b.model_dump(exclude_none=True) for b in resp.content]})
        tool_uses = [b for b in resp.content if b.type == "tool_use"]
        text_parts = [b.text for b in resp.content if b.type == "text"]
        if text_parts:
            print(f"\n[turn {turns}] " + " ".join(text_parts)[:600], flush=True)
        if not tool_uses:
            session.summary = "\n".join(text_parts)
            break

        results: list[dict] = []
        halted = False
        for tu in tool_uses:
            toolset = getattr(tu, "toolset_name", None)
            if toolset == "computer":
                if halted:
                    results.append({"type": "tool_result", "tool_use_id": tu.id, "toolset_name": "computer", "is_error": True,
                                    "content": "Not executed: an earlier computer action in this turn failed."})
                    continue
                if session.steps >= args.max_steps:
                    results.append({"type": "tool_result", "tool_use_id": tu.id, "toolset_name": "computer", "is_error": True,
                                    "content": "Step budget exhausted. Write your summary now."})
                    continue
                session.steps += 1
                try:
                    if tu.name in ("screenshot", "zoom"):
                        time.sleep(SETTLE_SEC)
                        b64, idx = screen.grab(tu.input.get("region") if tu.name == "zoom" else None)
                        hung = measure_hang(app.hwnd)
                        note = f"shot={idx}" + (f"; UI was hung for {hung:.1f}s" if hung > 0.2 else "; UI responsive")
                        results.append({"type": "tool_result", "tool_use_id": tu.id, "toolset_name": "computer",
                                        "content": [{"type": "text", "text": note},
                                                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64}}]})
                    else:
                        t0 = time.perf_counter()
                        text = execute_action(tu.name, tu.input, screen)
                        time.sleep(0.15)
                        hung = measure_hang(app.hwnd)
                        if hung > 0.2:
                            session.hangs.append((session.steps, hung))
                            text += f" — UI was hung for {hung:.1f}s after this action" + (" (still hung!)" if hung >= HANG_MAX_WAIT_SEC else "")
                        else:
                            text += f" — UI responsive ({(time.perf_counter() - t0) * 1000:.0f} ms)"
                        results.append({"type": "tool_result", "tool_use_id": tu.id, "toolset_name": "computer",
                                        "content": [{"type": "text", "text": text}]})
                    print(f"  step {session.steps}: {tu.name} {json.dumps(tu.input)[:80]}", flush=True)
                except pyautogui.FailSafeException:
                    session.summary = "Aborted by fail-safe (mouse moved to screen corner)."
                    return finish(session, report_dir, scenarios, app, args)
                except Exception as e:  # keep the loop alive, tell Claude what broke
                    halted = True
                    results.append({"type": "tool_result", "tool_use_id": tu.id, "toolset_name": "computer", "is_error": True,
                                    "content": f"Action failed: {e}"})
            elif tu.name == "report_finding":
                f = Finding(**tu.input)
                session.findings.append(f)
                print(f"  ! FINDING [{f.severity}/{f.category}] {f.title}", flush=True)
                results.append({"type": "tool_result", "tool_use_id": tu.id, "content": f"Recorded finding #{len(session.findings)}."})
            elif tu.name == "note_scenario":
                session.scenarios.append(ScenarioNote(**tu.input))
                results.append({"type": "tool_result", "tool_use_id": tu.id, "content": "Noted."})
            else:
                results.append({"type": "tool_result", "tool_use_id": tu.id, "is_error": True, "content": f"Unknown tool {tu.name}"})
        messages.append({"role": "user", "content": results})

    if not session.summary:
        session.summary = f"Stopped after {session.steps} actions (budget {args.max_steps})."
    return finish(session, report_dir, scenarios, app, args)


# --------------------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------------------

def finish(session: Session, report_dir: Path, scenarios: list[str], app: AppHandle, args: argparse.Namespace) -> Session:
    (report_dir / "findings.json").write_text(json.dumps({
        "model": MODEL, "steps": session.steps, "findings": [asdict(f) for f in session.findings],
        "scenarios": [asdict(s) for s in session.scenarios], "hangs": session.hangs,
        "usage": {"input": session.input_tokens, "output": session.output_tokens, "cache_read": session.cache_read},
        "summary": session.summary,
    }, indent=2, ensure_ascii=False), encoding="utf-8")
    (report_dir / "report.md").write_text(render_report(session, scenarios, report_dir), encoding="utf-8")
    print(f"\nReport: {report_dir / 'report.md'}")
    if args.file_issues:
        file_issues(session, report_dir, args)
    if app.proc and not args.keep_open:
        try:
            app.proc.terminate()
        except Exception:
            pass
    return session


def render_report(s: Session, scenarios: list[str], report_dir: Path) -> str:
    sev_order = {"critical": 0, "major": 1, "minor": 2}
    findings = sorted(s.findings, key=lambda f: sev_order.get(f.severity, 9))
    counts = {k: sum(1 for f in findings if f.severity == k) for k in ("critical", "major", "minor")}
    verdicts = {k: sum(1 for n in s.scenarios if n.verdict == k) for k in ("pass", "fail", "blocked", "skipped")}
    worst_hang = max((h for _, h in s.hangs), default=0.0)
    lines = [
        f"# Lunima UX test — {report_dir.name}",
        "",
        f"Model `{MODEL}`, {s.steps} UI actions, {len(scenarios)} scenarios "
        f"(pass {verdicts['pass']} / fail {verdicts['fail']} / blocked {verdicts['blocked']} / skipped {verdicts['skipped']}).",
        f"Findings: {counts['critical']} critical, {counts['major']} major, {counts['minor']} minor. "
        f"UI hangs measured: {len(s.hangs)} (worst {worst_hang:.1f}s). "
        f"Tokens: {s.input_tokens:,} in ({s.cache_read:,} cached) / {s.output_tokens:,} out.",
        "",
        "## Summary",
        "",
        s.summary or "(none)",
        "",
        "## Findings",
        "",
    ]
    for i, f in enumerate(findings, 1):
        shot = f"![shot {f.screenshot}](shots/shot-{f.screenshot:03d}.png)" if f.screenshot else "(no screenshot)"
        lines += [
            f"### {i}. [{f.severity.upper()} · {f.category}] {f.title}",
            "",
            f"**Where:** {f.where}",
            "",
            f"**Steps:**\n{f.steps}",
            "",
            f"**Expected:** {f.expected}",
            "",
            f"**Observed:** {f.observed}",
            "",
            f"**Suggestion:** {f.suggestion}",
            "",
            shot,
            "",
        ]
    lines += ["## Scenario verdicts", ""]
    for n in s.scenarios:
        lines.append(f"- **{n.verdict.upper()}** — {n.scenario}: {n.observation}")
    if s.hangs:
        lines += ["", "## Measured UI hangs", ""] + [f"- step {st}: {sec:.1f}s" for st, sec in s.hangs]
    return "\n".join(lines) + "\n"


def file_issues(s: Session, report_dir: Path, args: argparse.Namespace) -> None:
    """Commit screenshots to a branch of the Lunima clone and open one issue per finding."""
    sev_rank = {"minor": 0, "major": 1, "critical": 2}
    picked = [f for f in s.findings if sev_rank[f.severity] >= sev_rank[args.min_severity]]
    if not picked:
        print("No findings at or above the severity threshold — no issues filed.")
        return
    clone = Path(args.lunima_clone)
    branch = f"ux-findings/{report_dir.name}"
    dest = clone / "docs" / "ux-findings" / report_dir.name
    dest.mkdir(parents=True, exist_ok=True)
    for f in picked:
        if f.screenshot:
            src = report_dir / "shots" / f"shot-{f.screenshot:03d}.png"
            if src.exists():
                (dest / src.name).write_bytes(src.read_bytes())
    (dest / "report.md").write_text((report_dir / "report.md").read_text(encoding="utf-8"), encoding="utf-8")
    g = lambda *a: subprocess.run(["git", "-C", str(clone), *a], check=True, capture_output=True, text=True)
    g("fetch", "origin", args.base_branch)
    g("checkout", "-B", branch, f"origin/{args.base_branch}")
    g("add", str(dest))
    g("commit", "-m", f"ux-tester: findings {report_dir.name} (screenshots + report)")
    g("push", "-u", "origin", branch)
    sha = g("rev-parse", "HEAD").stdout.strip()
    for f in picked:
        img = ""
        if f.screenshot:
            img = f"\n\n![screenshot](https://github.com/{args.repo}/blob/{sha}/docs/ux-findings/{report_dir.name}/shot-{f.screenshot:03d}.png?raw=true)"
        body = (f"**Category:** {f.category} · **Severity:** {f.severity} · **Where:** {f.where}\n\n"
                f"## Steps to reproduce\n{f.steps}\n\n## Expected\n{f.expected}\n\n## Observed\n{f.observed}\n\n"
                f"## Suggested fix (UX)\n{f.suggestion}{img}\n\n"
                f"_Found by the computer-use UX tester (model `{MODEL}`) walking the release checklist on a real desktop session. "
                f"Full report: `docs/ux-findings/{report_dir.name}/report.md` on branch `{branch}`._")
        out = subprocess.run(["gh", "issue", "create", "--repo", args.repo, "--title", f"UX: {f.title}", "--body", body],
                             capture_output=True, text=True)
        url = out.stdout.strip()
        if url:
            num = url.rsplit("/", 1)[-1]
            for label in ("ux-finding", *(["agent-task"] if args.label_agent_task else [])):
                subprocess.run(["gh", "api", f"repos/{args.repo}/issues/{num}/labels", "-X", "POST", "-f", f"labels[]={label}"],
                               capture_output=True, text=True)
            print(f"  filed {url}  [{f.severity}] {f.title}")
        else:
            print(f"  FAILED to file '{f.title}': {out.stderr.strip()[:200]}")


# --------------------------------------------------------------------------------------

def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--app", help="Path to CAP.Desktop.exe (or a .dll, launched with dotnet)")
    p.add_argument("--attach", action="store_true", help="Use an already running Lunima window instead of launching")
    p.add_argument("--window-title", default=WINDOW_TITLE_HINT, help="Substring of the app window title to wait for")
    p.add_argument("--checklist", help="docs/RELEASE-CHECKLIST.md to derive scenarios from")
    p.add_argument("--scenarios", help="Extra scenarios, one per line")
    p.add_argument("--include-auto", action="store_true", help="Also walk checklist items marked (auto)")
    p.add_argument("--limit", type=int, help="Only the first N scenarios")
    p.add_argument("--max-steps", type=int, default=120, help="Budget of UI actions")
    p.add_argument("--report-dir", default=str(Path(__file__).parent / "reports"))
    p.add_argument("--keep-open", action="store_true", help="Leave the app running afterwards")
    p.add_argument("--file-issues", action="store_true", help="Open GitHub issues for findings")
    p.add_argument("--min-severity", choices=["minor", "major", "critical"], default="major")
    p.add_argument("--label-agent-task", action="store_true", help="Also label filed issues agent-task")
    p.add_argument("--repo", default="aignermax/Lunima")
    p.add_argument("--lunima-clone", help="Local Lunima clone used to publish screenshots (for --file-issues)")
    p.add_argument("--base-branch", default="dev-ki")
    p.add_argument("--publish-report", metavar="REPORT_DIR",
                   help="Skip testing; file issues from an existing report directory (implies --file-issues)")
    args = p.parse_args()
    if args.publish_report:
        args.file_issues = True
    if args.file_issues and not args.lunima_clone:
        p.error("--file-issues needs --lunima-clone")
    if args.publish_report:
        publish_existing(Path(args.publish_report), args)
        return
    s = run(args)
    print(f"\nDone: {s.steps} actions, {len(s.findings)} findings, {len(s.hangs)} hangs.")


def publish_existing(report_dir: Path, args: argparse.Namespace) -> None:
    """Re-hydrate a saved session from findings.json and file issues for it."""
    data = json.loads((report_dir / "findings.json").read_text(encoding="utf-8"))
    s = Session(findings=[Finding(**f) for f in data["findings"]],
                scenarios=[ScenarioNote(**n) for n in data["scenarios"]],
                hangs=[tuple(h) for h in data.get("hangs", [])], steps=data.get("steps", 0),
                summary=data.get("summary", ""))
    file_issues(s, report_dir, args)


if __name__ == "__main__":
    main()
