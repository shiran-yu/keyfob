<div align="center">

# keyfob

**Keep your API keys and tokens out of Antigravity CLI, Claude Code and Codex conversations.**

Store a token once, let a command receive it for one run, and export nothing into your shell. Claude Code
includes a masked `/keyfob` panel; Antigravity CLI and Codex use the native CLI, skill, and shell guard.

[![version](https://img.shields.io/badge/version-0.2.1-blue)](https://github.com/shiran-yu/keyfob/releases)
[![Antigravity CLI](https://img.shields.io/badge/Antigravity_CLI-plugin-4285F4)](https://antigravity.google)
[![Claude Code](https://img.shields.io/badge/Claude_Code-plugin-D97757)](https://github.com/anthropics/claude-code)
[![Codex](https://img.shields.io/badge/Codex-plugin-111827)](https://developers.openai.com/plugins/build/plugins)
[![platforms](https://img.shields.io/badge/macOS_%C2%B7_Linux-arm64_%C2%B7_x86__64-2ea44f)](https://github.com/shiran-yu/keyfob/releases)
[![licence](https://img.shields.io/badge/licence-CC_BY--NC--SA_4.0-lightgrey)](LICENSE)

![In Claude Code: a token pasted into the chat is stored and replaced by "[stored in keyfob]" before the model sees it; then Claude asks for the Semantic Scholar key by name, the /keyfob panel opens on it, the key goes into its field and is redrawn as bullets, and Claude carries on with keyfob run. All keys in the recording are made up.](assets/claude-code.gif)

</div>

## Install

### Antigravity CLI (`agy`)

From local clone or checkout:

```sh
agy plugin install ./plugins/keyfob
```

Or install from marketplace/remote. The plugin registers the `usage` skill and the `run_command` guard hook automatically.

### Codex

```sh
git clone https://github.com/shiran-yu/keyfob.git
cd keyfob
codex plugin marketplace add "$PWD"
codex plugin add keyfob@keyfob-local
```

Start a new Codex thread, then review and trust the plugin hooks when prompted. The hooks run locally and
therefore are not available in Codex Cloud or the public universal plugin directory. The launcher downloads
the matching release binary once and verifies its SHA-256.

### Claude Code

Tell Claude Code:

```
Install https://github.com/shiran-yu/keyfob
```

It adds the plugin and asks you to restart. The keyfob binary comes with it: fetched once for your machine
and checked against the release's SHA-256.

<details>
<summary>What Claude runs</summary>

1. `claude plugin marketplace add shiran-yu/keyfob`
2. `claude plugin install keyfob@keyfob`
3. Ask the user to restart Claude Code. On first use the plugin's launcher downloads
   `keyfob-<target>.tar.gz` from the matching release and checks it against its `.sha256`.

</details>

On its own, for a server or a script: download `keyfob-<target>.tar.gz` from
[Releases](https://github.com/shiran-yu/keyfob/releases) and check it against its `.sha256`, or
`cargo install --git https://github.com/shiran-yu/keyfob`.

## Why

A token pasted into an agent chat can reach the model and local transcript. A key exported in `~/.bashrc`
sits in the environment of every process you start, and one `env` or `cat .env` can print it into the
conversation. keyfob keeps the value
in the OS keystore (or a `0600` file where there is none) and hands it to one command at a time, so the
model never needs to see it. What this covers, and what it does not, is [below](#what-it-protects-and-what-it-does-not).

## In Claude Code

- **Paste a token and it is gone before the model sees it.** Send `keyfob: s2-key <token>` on a line of its
  own (optionally followed by the variable name, e.g. `S2_API_KEY`) and the plugin stores the value before
  the message is kept; the model and the transcript get `keyfob: s2-key [stored in keyfob]`. A bare token in
  a format it recognises (OpenAI, Anthropic, GitHub, Hugging Face, Slack, Google, JWT, AWS access key IDs,
  Stripe live keys) is stored as `pasted-1`, `pasted-2`, … and replaced by `[keyfob:pasted-1]`. Anything
  else, such as a password or a value with a space, is sent as typed: use `/keyfob` for those.
- **Claude asks for a key by name.** It calls `keyfob_request`; `/keyfob` opens on that secret with a field
  that redraws what you paste as bullets, you press store, and a prompt tells Claude to go on. The value
  never becomes a message. You can open `/keyfob` yourself at any time.
- **A guard on Claude's Bash.** Commands that would print a secret (`keyfob get`, `env`, `echo $TOKEN`,
  reading the store), carry a literal token, or need a group's secrets without `keyfob run` are refused
  with the fix. It splits the command as a shell does, so a word inside a quoted grep pattern is not taken
  for `keyfob get`, `env` and the like.
- **Plugins say what they need.** A plugin lists its keys in a `keyfob.json`; keyfob finds it in every
  plugin enabled in your user settings, `/keyfob` shows what is missing and where to get it, and
  `keyfob check --live` asks each service a group declares a check for whether its keys still work.

## In Codex

- **The same skill and shell guard work locally.** Codex exposes shell calls to `PreToolUse` as `Bash`, so
  commands that print stored values, carry literal tokens, read the store, or omit a required `keyfob run`
  wrapper are refused before execution.
- **Recognised secrets in a prompt are stored, then that prompt is blocked.** Codex cannot rewrite a submitted
  prompt today. After keyfob captures the value, resend the request without it. Prefer running
  `keyfob add <name>` in your own terminal, especially for passwords and values containing spaces.
- **Credential requests use the CLI.** Codex records a missing credential with `keyfob request`; you add it in
  your terminal and Codex verifies only its name with `keyfob ls --json`. Codex does not provide Claude's
  `/keyfob` panel, `keyfob_request`, or `keyfob_status` module tools.
- **Destructive keyfob commands stay user-owned.** Codex `PreToolUse` does not currently support an `ask`
  decision, so `keyfob rm` and `migrate --apply` are blocked for the agent and must be run in your terminal.

Codex does not trust plugin hooks automatically. Prompt capture and the command guard start only after you
review and trust `hooks/codex.json`; without that trust, use terminal entry and do not paste secrets into chat.

## In Antigravity CLI (`agy`)

- **PreToolUse Guard on `run_command`.** Antigravity CLI executes shell commands via `run_command`. Keyfob intercepts `run_command` before execution to block commands that would print secrets (`keyfob get`, `env`, `echo $TOKEN`, directly reading the store file), carry literal tokens, or execute group-protected tools without `keyfob run`.
- **Interactive approval for deletions.** Antigravity CLI natively supports interactive permission prompts via `{"decision": "ask"}` in its PreToolUse lifecycle, so commands like `keyfob rm` or `migrate --apply` prompt you for confirmation before executing.
- **Plugin declarations discovery.** keyfob automatically discovers `keyfob.json` files in workspace plugins (`.agents/plugins/<plugin>/` or `.agent/plugins/<plugin>/`), installed global plugins (`~/.gemini/config/plugins/<plugin>/`), and personal declarations in `~/.config/keyfob/declarations.d/`.
- **Doctor auditing.** `keyfob doctor` inspects Antigravity MCP server configurations (`~/.gemini/config/mcp_config.json`, project `mcp_config.json`, `.agents/mcp_config.json`) and agent settings for plaintext secrets.

## In your terminal

![keyfob doctor finds an API key written in plain text in ~/.bashrc; keyfob add stores it without echoing it; keyfob ls lists names, never values; keyfob run hands it to eval.py for that one run; echo shows the shell itself never had it. The key is made up.](assets/terminal.gif)

```sh
keyfob add openai-key --env OPENAI_API_KEY      # asks without echo in your terminal
keyfob run openai-key -- python3 eval.py        # the script reads os.environ["OPENAI_API_KEY"]
keyfob run binance -- binance-cli spot ping     # a group a plugin declared: several variables at once
keyfob ls                                       # names, variables, who needs them; never values
keyfob check --live                             # complete? still accepted by the service?
keyfob doctor                                   # storage in use; plaintext secrets left in rc files or agent settings
```

| Command | Does |
|---|---|
| `add <name> [--env VAR] [--note T] [--from-env VAR \| --stdin]` | store; asks on the terminal by default |
| `request <name> [--env VAR] --reason T [--obtain URL]` | record that a secret is wanted (no value) in `~/.config/keyfob/declarations.d/requested.json`; `/keyfob` lists it to add |
| `run <group\|name\|name=VAR>... -- <cmd>` | run `cmd` with those secrets in its environment (`env` is an alias) |
| `ls`, `check [--live]`, `which VAR`, `decls`, `doctor` | look, never show values; `--json` on all but `which` |
| `get <name>` | print a value: your terminal, your programs (the agent shell guard refuses it) |
| `rm <name>`, `rename <old> <new>` | deletion requires the user; rename does not expose the value |
| `migrate --keychain-prefix <old/> [--names a,b] [--apply]` | preview, then with `--apply` copy Keychain items another tool wrote as `<old/><name>`, for the declared names or `--names` |
| `scrub` | stdin to stdout with every stored value replaced by `[keyfob:<name>]` |
| `capture`, `patterns`, `guard` | used by the Antigravity CLI, Claude Code and Codex plugins |

**Where the values live.** The macOS Keychain, the Linux Secret Service, or, on a server or cluster with
neither, one `0600` file in a `0700` folder (what Claude Code itself does for its own login on Linux). Choose
with `KEYFOB_BACKEND` or `"backend"` in `~/.config/keyfob/config.json` (`keychain`, `secret-service`, `file`,
`auto`). On macOS keyfob goes through Apple's `/usr/bin/security`, so the Keychain's access list stays the same
across keyfob upgrades (no "allow access" dialog after each) and values travel on pipes, never in an argv.
One self-contained binary (Rust) for macOS (arm64, x86_64) and Linux (x86_64, aarch64; static musl builds).

## For plugin authors: keyfob.json

Put it at your plugin's root. keyfob reads it from enabled Claude Code plugins, installed Codex plugin
caches, and Antigravity plugins (`~/.gemini/config/plugins/` or `.agents/plugins/`); people add their own in
`~/.config/keyfob/declarations.d/`. Schema: [`plugins/keyfob/schema/keyfob.schema.json`](plugins/keyfob/schema/keyfob.schema.json).

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

A group may list `"optional": ["S2_API_KEY"]`: those variables are passed when stored and skipped when not,
so `keyfob run scholar --` runs whether or not the person has that key.

Your scripts read the variables; your docs say `keyfob run scholar -- <command>`. A local (stdio) MCP server
that needs a key is started as `keyfob run <name> -- <server command>` in its MCP config; credentials in a
remote server's headers are outside what keyfob can hand over.

## What it protects, and what it does not

- It keeps tokens out of rc files, settings files and the environment of every process. With trusted local
  hooks, it intercepts a `keyfob:` line or recognised token before the model receives it. Claude rewrites the
  prompt; Codex stores the value and blocks that submission so the user can resend without it.
- It does not filter what a command run under `keyfob run` prints: a script that prints its key puts it in the
  conversation. A literal token that an agent writes into a Bash command is already in the transcript when the
  guard refuses it; the refusal stops it from running, not from being written.
- The guard is a seatbelt against mistakes, not a sandbox: a command written to get around it can. It fails open
  if it cannot read its input.
- Whatever runs as you can read what you can read: on a server the file store protects against other users, not
  against root; on macOS any program running as you can read keyfob's items through `/usr/bin/security` without
  a dialog, and the Keychain asks only apps that read it directly.
- In Claude Code, a pasted `keyfob:` line or recognised token is stored and replaced before the message enters
  the session. In Codex, it is stored and the prompt is blocked. Other text is sent as typed. Claude Code's own
  prompt history file
  (`~/.claude/history.jsonl`) is written by the terminal, and is not covered unless that turns out otherwise on
  your version: check with a dummy token, and use `/keyfob` (its input never becomes a message) when it matters.
- The `/keyfob` field redraws its content as bullets after each keystroke or paste, so what you type can show for
  a moment before it is redrawn.

## Develop

`cargo test` (add `KEYFOB_TEST_KEYCHAIN=1` on macOS for the Keychain round trip),
`python3 -m unittest discover -s tests -v`, `claude plugin validate plugins/keyfob`, and
`claude plugin test plugins/keyfob`. In a checkout the launcher uses `target/release` or
`target/debug`. Releases: bump `version` in `Cargo.toml`, `plugin.json`, `VERSION=` in
`plugins/keyfob/bin/keyfob` and the README badge, run `cargo build` so `Cargo.lock` follows (the release build
uses `--locked`), commit, then push a `v<version>` tag.

Licensed under [CC BY-NC-SA 4.0](LICENSE): free to use, share and adapt with attribution, for non-commercial purposes, under the same license. Version 0.1.0 was published under MIT.
