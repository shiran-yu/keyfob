"""Shared environment setup for the Codex hook adapters."""

import glob
import os


def keyfob_env():
    env = dict(os.environ)
    codex_home = env.get("CODEX_HOME") or os.path.join(os.path.expanduser("~"), ".codex")
    patterns = (
        os.path.join(codex_home, "plugins", "cache", "*", "*", "*", "keyfob.json"),
        os.path.join(codex_home, "plugins", "cache", "*", "*", "*", ".codex-plugin", "keyfob.json"),
    )
    paths = []
    existing = env.get("KEYFOB_DECLARATIONS")
    if existing:
        paths.extend(path for path in existing.split(os.pathsep) if path)
    for pattern in patterns:
        paths.extend(sorted(glob.glob(pattern)))
    plugin_root = env.get("PLUGIN_ROOT")
    if plugin_root:
        for relative in ("keyfob.json", os.path.join(".codex-plugin", "keyfob.json")):
            candidate = os.path.join(plugin_root, relative)
            if os.path.isfile(candidate):
                paths.append(candidate)
    if paths:
        env["KEYFOB_DECLARATIONS"] = os.pathsep.join(dict.fromkeys(paths))
    env["KEYFOB_HOOK_HOST"] = "codex"
    return env
