import importlib.util
import os
import sys
import unittest


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOOKS = os.path.join(ROOT, "plugins", "keyfob", "hooks")
sys.path.insert(0, HOOKS)
PATH = os.path.join(HOOKS, "codex_guard.py")
SPEC = importlib.util.spec_from_file_location("codex_guard", PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class GuardAdapterCase(unittest.TestCase):
    def test_allow_stays_silent(self):
        self.assertIsNone(MODULE.adapt(""))

    def test_deny_is_preserved(self):
        source = '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"blocked"}}'
        result = MODULE.adapt(source)
        self.assertEqual(result["hookSpecificOutput"]["permissionDecision"], "deny")
        self.assertEqual(result["hookSpecificOutput"]["permissionDecisionReason"], "blocked")

    def test_ask_becomes_deny(self):
        source = '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"Delete it?"}}'
        result = MODULE.adapt(source)
        hook = result["hookSpecificOutput"]
        self.assertEqual(hook["permissionDecision"], "deny")
        self.assertIn("run this command yourself", hook["permissionDecisionReason"])

    def test_invalid_output_fails_open(self):
        self.assertIsNone(MODULE.adapt("not json"))


if __name__ == "__main__":
    unittest.main()
