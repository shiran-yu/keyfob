#!/usr/bin/env python3
"""Adapt keyfob's Claude-compatible PreToolUse output to Codex."""

import json
import subprocess
import sys

from codex_common import keyfob_env


def adapt(output):
    if not output.strip():
        return None
    try:
        value = json.loads(output)
    except ValueError:
        return None
    hook = value.get("hookSpecificOutput") if isinstance(value, dict) else None
    if not isinstance(hook, dict):
        return None
    if hook.get("permissionDecision") == "ask":
        reason = hook.get("permissionDecisionReason")
        if not isinstance(reason, str):
            reason = "keyfob requires confirmation for this command."
        hook["permissionDecision"] = "deny"
        hook["permissionDecisionReason"] = reason + " Codex hooks cannot request approval yet; run this command yourself in a terminal if you want to proceed."
    return value


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    keyfob = argv[0] if argv else "keyfob"
    event = sys.stdin.read()
    try:
        completed = subprocess.run(
            [keyfob, "guard"],
            input=event,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=55,
            check=False,
            env=keyfob_env(),
        )
    except (OSError, subprocess.SubprocessError):
        return 0
    result = adapt(completed.stdout) if completed.returncode == 0 else None
    if result is not None:
        json.dump(result, sys.stdout, separators=(",", ":"))
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
