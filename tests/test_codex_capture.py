import importlib.util
import json
import os
import subprocess
import sys
import unittest


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOOKS = os.path.join(ROOT, "plugins", "keyfob", "hooks")
sys.path.insert(0, HOOKS)
PATH = os.path.join(HOOKS, "codex_capture.py")
SPEC = importlib.util.spec_from_file_location("codex_capture", PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class Completed:
    def __init__(self, report, returncode=0):
        self.stdout = json.dumps(report)
        self.returncode = returncode


class CaptureHookCase(unittest.TestCase):
    def test_ordinary_prompt_does_not_start_keyfob(self):
        def unexpected(*args, **kwargs):
            self.fail("runner should not be called")

        self.assertIsNone(MODULE.handle({"prompt": "please run the tests"}, "keyfob", unexpected))

    def test_named_secret_is_stored_then_prompt_is_blocked(self):
        token = "sk-" + "a" * 32

        def capture(argv, **kwargs):
            self.assertEqual(argv, ["keyfob", "capture"])
            self.assertIn(token, kwargs["input"])
            self.assertEqual(kwargs["env"]["KEYFOB_HOOK_HOST"], "codex")
            return Completed({"stored": [{"name": "openai-key"}], "pending": [], "failed": []})

        result = MODULE.handle({"prompt": "keyfob: openai-key " + token}, "keyfob", capture)
        self.assertEqual(result["decision"], "block")
        self.assertIn("openai-key", result["reason"])
        self.assertNotIn(token, result["reason"])

    def test_recognised_bare_token_is_blocked(self):
        token = "ghp_" + "a" * 36
        result = MODULE.handle(
            {"prompt": "use " + token},
            "keyfob",
            lambda *args, **kwargs: Completed({"stored": [], "pending": [{"name": "pasted-1"}], "failed": []}),
        )
        self.assertIn("pasted-1", result["reason"])
        self.assertNotIn(token, result["reason"])

    def test_capture_failure_fails_closed_without_echoing_prompt(self):
        token = "sk-ant-" + "a" * 24

        def fail(*args, **kwargs):
            raise subprocess.TimeoutExpired("keyfob", 55)

        result = MODULE.handle({"prompt": token}, "keyfob", fail)
        self.assertEqual(result["decision"], "block")
        self.assertNotIn(token, result["reason"])


if __name__ == "__main__":
    unittest.main()
