#!/usr/bin/env python3
"""Codex UserPromptSubmit adapter for keyfob.

Codex can block a prompt but cannot replace it. When a prompt contains a
high-confidence token, store it with `keyfob capture`, block that submission,
and ask the user to resend the prompt without the value.
"""

import json
import re
import subprocess
import sys

from codex_common import keyfob_env


PATTERNS = tuple(
    re.compile(pattern)
    for pattern in (
        r"(?m)^[ \t]*keyfob:[ \t]*[a-z0-9][a-z0-9._-]{0,63}[ \t]+\S+(?:[ \t]+[A-Za-z_][A-Za-z0-9_]*)?[ \t]*$",
        r"sk-ant-[A-Za-z0-9_-]{20,}",
        r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}",
        r"\bgh[pousr]_[A-Za-z0-9]{36,}",
        r"\bgithub_pat_[A-Za-z0-9_]{22,}",
        r"\bhf_[A-Za-z0-9]{30,}",
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"\bAIza[0-9A-Za-z_-]{35}\b",
        r"\b[rs]k_live_[0-9A-Za-z]{20,}",
        r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
    )
)


def looks_sensitive(prompt):
    return any(pattern.search(prompt) for pattern in PATTERNS)


def blocked(reason):
    return {"decision": "block", "reason": reason}


def handle(event, keyfob, runner=subprocess.run):
    prompt = event.get("prompt") if isinstance(event, dict) else None
    if not isinstance(prompt, str) or not looks_sensitive(prompt):
        return None

    try:
        completed = runner(
            [keyfob, "capture"],
            input=prompt,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=55,
            check=False,
            env=keyfob_env(),
        )
        report = json.loads(completed.stdout) if completed.returncode == 0 else None
    except (OSError, subprocess.SubprocessError, ValueError, TypeError):
        report = None

    if not isinstance(report, dict):
        return blocked("keyfob detected a likely secret but could not store it. Run `keyfob add <name>` in your terminal, then resend this prompt without the value.")

    saved = []
    for field in ("stored", "pending"):
        for item in report.get(field, []):
            name = item.get("name") if isinstance(item, dict) else None
            if isinstance(name, str) and name not in saved:
                saved.append(name)
    failed = [item for item in report.get("failed", []) if isinstance(item, dict)]

    if saved and not failed:
        names = ", ".join(saved)
        return blocked("keyfob stored the secret as %s. Resend the request without the secret value; use `keyfob run` when a command needs it." % names)
    if saved:
        names = ", ".join(saved)
        return blocked("keyfob stored %s, but another detected value could not be stored. Add the remaining secret in your terminal, then resend without any secret values." % names)
    return blocked("keyfob detected a likely secret but did not store it. Run `keyfob add <name>` in your terminal, then resend this prompt without the value.")


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    keyfob = argv[0] if argv else "keyfob"
    try:
        event = json.load(sys.stdin)
    except (ValueError, TypeError):
        return 0
    result = handle(event, keyfob)
    if result is not None:
        json.dump(result, sys.stdout, separators=(",", ":"))
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
