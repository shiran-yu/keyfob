---
name: usage
description: Use whenever a command, script or MCP server needs an API key, token or password, when one fails with 401/403 or "missing key", when the user pastes or mentions a token, asks where their keys are, wants to add, rotate or remove one, or when a plugin's keyfob.json is being written. keyfob keeps values in the OS keystore (or a 0600 file on servers) and hands them to one command at a time without exposing them to the agent.
---

# keyfob: using secrets without seeing them

The packaged launcher is `../../bin/keyfob` relative to this skill. Resolve it to an absolute path before
running commands; use a `keyfob` already on `PATH` only when it is the same installation.

## Rules

1. **Run commands that need a secret under keyfob**: `keyfob run <group|name|name=VAR>... -- <command>`.
   The command gets the values as environment variables for its own run; nothing is exported anywhere else.
   - a group a plugin declared: `keyfob run binance -- binance-cli futures-usds futures-account-balance-v3`
   - one secret, its declared variable: `keyfob run openai-key -- python3 eval.py`
   - one secret, a variable of your choosing: `keyfob run s2-key=S2_API_KEY -- python3 search.py`
   - several: `keyfob run t212-isa binance -- python3 collector.py`
2. **Never put a value where it would be printed or kept**: no `keyfob get`, no `env`/`printenv`, no
   `echo $TOKEN`, no literal token in a command or a file. The guard refuses these and says what to do
   instead; do that. To test that a variable is set: `[ -n "$VAR" ] && echo set`.
3. **Scripts read secrets from the environment** (`os.environ["S2_API_KEY"]`), never from a file path or an
   argument, and never print them.
4. **The user adds a secret, not you. Never ask for its value in normal chat.** Use a stable name such as
   `openreview-password`, `github-token`, or `openalex-key`; reuse a name `keyfob ls --json` already lists.
   - **In Antigravity CLI (`agy`)**, run `keyfob request <name> --env <VAR> --reason <text> [--obtain <URL>]`,
     then ask the user to run `keyfob add <name> --env <VAR>` in their own terminal. After they confirm it is stored,
     verify with `keyfob ls --json` and proceed under `keyfob run`. The PreToolUse hook guards `run_command` calls
     against token exposure and enforces `keyfob run` for declared groups.
   - **In Codex**, run `keyfob request <name> --env <VAR> --reason <text> [--obtain <URL>]`, then ask the user
     to run `keyfob add <name> --env <VAR>` in their own terminal. After they say it is stored, verify with
     `keyfob ls --json` and continue under `keyfob run`. When the user has explicitly trusted the plugin hooks,
     a one-line `keyfob: <name> <token> [VAR]` prompt is also captured and blocked before the model sees it;
     they must then resend the request without the value. Terminal entry remains the preferred path.
   - **In Claude Code**, call `keyfob_request` with `secret`, `env`, `reason`, and optional `obtain`.
     `/keyfob` opens on that secret with a masked field and the tool returns at once:
   - `opened`: say in one line what to paste, then end your turn. When the person stores it, a prompt from
     keyfob says `<name> is now stored`; go on with `keyfob run <name>=<VAR> -- <command>`. If they put it
     off, the prompt says so: say what cannot run without it.
   - `stored`: it is already there; run the command as the result says.
   - `waiting`: the terminal is too narrow to open the panel; tell the person to type `/keyfob`.
     The person may also open `/keyfob` themselves or run `keyfob add <name>` in their own terminal.
   `keyfob add <name> --from-env VAR` is safe for the agent to run when the value is already in its environment.
5. **A recognised token pasted without a `keyfob:` line** is stored as `pasted-N`. In Claude Code the prompt
   is rewritten with `[keyfob:pasted-N]`; in Codex the prompt is blocked and must be resent without the value.
   Ask what it is for, then `keyfob rename pasted-N <name>`.
6. **Deleting** (`keyfob rm`) requires the user. Claude Code and Antigravity CLI ask for approval; Codex blocks it because
   PreToolUse cannot request approval yet, so tell the user to run the deletion in their own terminal.

## Finding out what exists

- In Claude Code, the `keyfob_status` tool is available. Across Antigravity CLI, Claude Code, and Codex, `keyfob ls --json`
  and `keyfob check --json` show names, variables, consumers, and complete groups without values.
  `keyfob check --live` asks each declared service whether its secrets work.
- `keyfob which <VAR>`: the group and secret behind a variable.
- `keyfob doctor`: the storage in use, and plaintext secrets left in shell rc files or agent settings (names
  only). For each: `keyfob add <name> --from-env VAR` (if it is in your environment), then the user deletes the
  line. An MCP server that read the variable from settings is then started as `keyfob run <name> -- <server>`.

## Declaring what a plugin needs: keyfob.json

At the plugin's root (personal ones in `~/.config/keyfob/declarations.d/<name>.json`). keyfob finds declarations
in enabled Claude Code plugins, installed Codex plugin caches, and Antigravity plugins (`~/.gemini/config/plugins/` or
workspace `.agents/plugins/`). Schema: `schema/keyfob.schema.json` here.

```json
{
  "name": "scholar-tools",
  "secrets": {
    "s2-key": { "env": "S2_API_KEY", "description": "Semantic Scholar API key",
                "obtain": "https://www.semanticscholar.org/product/api#api-key-form" }
  },
  "groups": {
    "scholar": {
      "env": { "S2_API_KEY": "s2-key", "OPENALEX_API_KEY": "openalex-key" },
      "require_for": ["(^|\\s)python3?\\s+\\S*scholar_search\\.py"],
      "check": { "type": "http", "url": "https://api.semanticscholar.org/graph/v1/paper/CorpusId:1",
                 "headers": { "x-api-key": "${S2_API_KEY}" } }
    }
  },
  "guard": { "deny_paths": ["~/.config/scholar/keys.env"] }
}
```

`require_for`: once the group is complete, a Bash command matching one of the regexes that is not run under
`keyfob run <group> --` is refused, with the fix in the message. `check`: `http` (with `${VAR}` in the URL or
headers, or `basic: [USER_VAR, PASS_VAR]`) or `command` (`argv`, exit 0 means the secrets work).
`optional`: variables of `env` that `keyfob run <group>` passes when stored and skips when not, so a script
that uses a key when it has one runs either way; they never make the group incomplete.

## Where values live

| Machine | Storage | Note |
|---|---|---|
| macOS | login Keychain, service `keyfob/<name>`, through `/usr/bin/security` | over SSH the Keychain is locked: `security unlock-keychain`, or `KEYFOB_BACKEND=file` |
| Linux desktop | Secret Service (GNOME Keyring, KWallet) | |
| Linux server, cluster | `~/.local/share/keyfob/secrets.json`, 0600 in a 0700 folder | what Claude Code uses for its own login on Linux; Slurm jobs can read it unattended |

Pick one with `KEYFOB_BACKEND` or `"backend"` in `~/.config/keyfob/config.json`. Keychain items another tool
wrote as `<prefix><name>` are read too after `"keychain_legacy_prefixes": ["<prefix>"]` in that file, or copied
with `keyfob migrate --keychain-prefix <prefix>`.
