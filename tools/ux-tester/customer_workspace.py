"""Fresh build workspaces and serialized runs; never modify the active worker clone."""
from __future__ import annotations

import contextlib
import json
import os
import subprocess
import time
from pathlib import Path


def command(args: list[str], cwd: Path | None = None, timeout: int = 120) -> str:
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=True,
                            encoding="utf-8", errors="replace", timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{args[0]} {args[1]} failed: {result.stderr[-2000:]}")
    return result.stdout.strip()


def gh_json(repo: str, args: list[str]):
    return json.loads(command(["gh", *args, "--repo", repo]))


def targets(repo: str, base: str, label: str) -> list[dict]:
    prs = gh_json(repo, ["pr", "list", "--state", "open", "--base", base, "--limit", "100",
                         "--json", "number,title,headRefOid,isDraft,isCrossRepository,labels"])
    eligible = [p for p in prs if not p["isDraft"] and not p["isCrossRepository"]
                and (p["title"].startswith("Agent:") or any(l["name"] == label for l in p["labels"]))]
    baseline = json.loads(command(["gh", "api", f"repos/{repo}/commits/{base}"]))["sha"]
    return ([{"key": f"pr-{p['number']}", "pr": p["number"], "sha": p["headRefOid"],
              "ref": f"refs/pull/{p['number']}/head"} for p in sorted(eligible, key=lambda p: p["number"])]
            + [{"key": "baseline", "pr": None, "sha": baseline, "ref": f"refs/heads/{base}"}])


def prepare(repo: str, target: dict, run_dir: Path, project: str) -> Path:
    checkout = run_dir / "checkout"
    # Always a new directory. No reset/clean in the maintainer's or worker's tree.
    checkout.mkdir()
    command(["git", "init", str(checkout)])
    command(["git", "remote", "add", "origin", f"https://github.com/{repo}.git"], checkout)
    command(["git", "fetch", "--depth", "1", "origin", target["ref"]], checkout, 300)
    if command(["git", "rev-parse", "FETCH_HEAD"], checkout) != target["sha"]:
        raise RuntimeError("Branch advanced while fetching; retry with the new commit")
    command(["git", "checkout", "--detach", target["sha"]], checkout)
    project_path = (checkout / project).resolve()
    if not project_path.is_relative_to(checkout.resolve()) or not project_path.is_file():
        raise ValueError("customerProject must name a project inside the tested checkout")
    command(["dotnet", "build", str(project_path), "-c", "Debug", "--nologo"], checkout, 900)
    # Infer output path through MSBuild; no stale binary or guessed target framework.
    raw = command(["dotnet", "msbuild", str(project_path), "-p:Configuration=Debug", "-getProperty:TargetPath"], checkout)
    app = Path(raw.strip())
    if not app.is_absolute():
        app = checkout / app
    if not app.resolve().is_relative_to(checkout.resolve()) or not app.is_file():
        raise RuntimeError("Build did not produce an application in the isolated checkout")
    return app


@contextlib.contextmanager
def cycle_lock(path: Path):
    """OS-owned lock releases automatically after a crash; no stale claim files."""
    with path.open("a+b") as handle:
        handle.seek(0)
        if handle.read(1) == b"":
            handle.write(b"0")
            handle.flush()
        handle.seek(0)
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            yield
        finally:
            handle.seek(0)
            if os.name == "nt":
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def write_json(path: Path, data: dict) -> None:
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
    temporary.replace(path)


def fresh(record: dict, identity: dict, now: float, max_age: int) -> bool:
    age = now - record.get("finished_at", 0)
    ttl = max_age if record.get("status") in ("passed", "needs_changes") else min(max_age, 3600)
    return record.get("identity") == identity and 0 <= age < ttl
