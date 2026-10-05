"""Windows preflight and input boundaries for unattended customer sessions."""
from __future__ import annotations

import ctypes
import os
import re
from pathlib import Path


def require_unlocked_desktop() -> None:
    if os.name != "nt":
        raise RuntimeError("Customer desktop testing currently requires Windows")
    user = ctypes.windll.user32
    user.OpenInputDesktop.restype = ctypes.c_void_p
    handle = user.OpenInputDesktop(0, False, 0x0100)  # DESKTOP_SWITCHDESKTOP
    if not handle:
        raise RuntimeError("No interactive desktop; customer review is blocked")
    try:
        if not user.SwitchDesktop(ctypes.c_void_p(handle)):
            raise RuntimeError("Desktop is locked; customer review is blocked")
    finally:
        user.CloseDesktop(ctypes.c_void_p(handle))


def window_pid(hwnd: int) -> int:
    pid = ctypes.c_ulong()
    ctypes.windll.user32.GetWindowThreadProcessId(ctypes.c_void_p(hwnd), ctypes.byref(pid))
    return pid.value


def require_target_foreground(hwnd: int) -> None:
    require_unlocked_desktop()
    user = ctypes.windll.user32
    user.GetForegroundWindow.restype = ctypes.c_void_p
    foreground = user.GetForegroundWindow()
    if not foreground or window_pid(foreground) != window_pid(hwnd):
        raise RuntimeError("Another application has focus; stopping customer interaction")


def validate_action(name: str, inp: dict, screen, window) -> None:
    """Reject system shortcuts and coordinates outside the tested application."""
    require_target_foreground(window._hWnd)
    if name in ("key", "hold_key"):
        keys = re.split(r"[+\-]", inp.get("text", "").lower())
        if any(k in ("super", "win", "windows", "meta", "cmd") or k.startswith(("super_", "meta_", "win_")) for k in keys):
            raise ValueError("System shortcuts are unavailable to the customer role")
        if any(k.startswith("alt") for k in keys) and any(k in ("tab", "f4", "escape", "esc", "space") for k in keys):
            raise ValueError("Window-switching/closing shortcuts are unavailable")
        if any(k in ("delete", "del", "escape", "esc") for k in keys) and any(k.startswith(("ctrl", "control")) for k in keys):
            raise ValueError("System shortcuts are unavailable")
    if name in ("left_mouse_down", "left_mouse_up", "hold_key"):
        raise ValueError("Use bounded click/drag/key actions in customer sessions")
    if name.endswith("click") and inp.get("text"):
        # Click modifiers go through the same shortcut validation as keys.
        validate_action("key", {"text": inp["text"]}, screen, window)
    for field in ("coordinate", "start_coordinate"):
        if field in inp:
            x, y = screen.to_screen(inp[field])
            if not (window.left <= x < window.right and window.top <= y < window.bottom):
                raise ValueError("Input outside the target application is unavailable")


def app_environment(profile: Path) -> dict:
    """Keep app preferences and saved designs away from the user's normal profile."""
    env = os.environ.copy()
    for key, subdir in (("HOME", "home"), ("USERPROFILE", "home"), ("APPDATA", "roaming"),
                        ("LOCALAPPDATA", "local"), ("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data")):
        path = profile / subdir
        path.mkdir(parents=True, exist_ok=True)
        env[key] = str(path)
    # The desktop application does not need the runner's service credentials.
    for key in list(env):
        if any(word in key.upper() for word in ("TOKEN", "API_KEY", "SECRET", "PASSWORD")):
            env.pop(key)
    return env
