---
name: usage
description: Use whenever a command, script or MCP server needs an API key, token or password, when one fails with 401/403 or "missing key", when the user pastes or mentions a token, asks where their keys are, wants to add, rotate or remove one, or when a plugin's keyfob.json is being written. keyfob keeps the values (macOS Keychain, Linux Secret Service, or a 0600 file on servers) and hands them to one command at a time; Claude never sees them.
---

# keyfob: using secrets without seeing them

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
4. **The user adds a secret, not you; you ask for it with `keyfob_request`.** Call it with `secret` (the name,
   `<service>-<kind>`: openreview-password, github-token, openalex-key; reuse a name `keyfob_status` lists),
   `env` (the variable), `reason` (one line the person sees) and, when you know it, `obtain` (where to get
   one). `/keyfob` opens on that secret with a masked field and the tool returns at once:
   - `opened`: say in one line what to paste, then end your turn. When the person stores it, a prompt from
     keyfob says `<name> is now stored`; go on with `keyfob run <name>=<VAR> -- <command>`. If they put it
     off, the prompt says so: say what cannot run without it.
   - `stored`: it is already there; run the command as the result says.
   - `waiting`: the terminal is too narrow to open the panel; tell the person to type `/keyfob`.
   Never ask for a value in chat. The person may also open `/keyfob` themselves, run `keyfob add <name>` in
   their own terminal, or paste `keyfob: <name> <token>` (stored before the message is kept; a value with a
   space, or a password, is safer in the panel). `keyfob add <name> --from-env VAR` is yours to run when the
   value is already in your environment.
5. **A token pasted without the `keyfob:` line** is stored as `pasted-N` and replaced by `[keyfob:pasted-N]`;
   ask what it is for, then `keyfob rename pasted-N <name>`.
6. **Deleting** (`keyfob rm`) asks the user first. Do not try to work around it.

## Finding out what exists

- The `keyfob_status` tool, or `keyfob ls --json` / `keyfob check --json`: names, the variable each goes into,
  who needs it, which groups are complete. `keyfob check --live` asks each service whether its secrets work.
- `keyfob which <VAR>`: the group and secret behind a variable.
- `keyfob doctor`: the storage in use, and plaintext secrets left in shell rc files or Claude settings (names
  only). For each: `keyfob add <name> --from-env VAR` (if it is in your environment), then the user deletes the
  line. An MCP server that read the variable from settings is then started as `keyfob run <name> -- <server>`.

## Declaring what a plugin needs: keyfob.json

At the plugin's root (personal ones in `~/.config/keyfob/declarations.d/<name>.json`). keyfob finds the files
of every enabled plugin by itself. Schema: `schema/keyfob.schema.json` in this plugin.

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
