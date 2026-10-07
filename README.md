# keyfob

API tokens for Claude Code, your shell and your scripts, kept out of everyone's way.

- **Stored by the operating system**: the macOS Keychain, the Linux Secret Service, or, on a server or cluster
  with neither, one `0600` file in a `0700` folder (what Claude Code itself does for its own login on Linux).
- **Handed to one command at a time**: `keyfob run openai -- python3 eval.py`. Nothing is exported into your
  shell, so nothing else (and no model) can read it from the environment.
- **Pasted straight into the chat**: send `keyfob: s2-key <token>` to Claude Code and the plugin stores it before
  the message is kept; the model and the transcript get `keyfob: s2-key [stored in keyfob]`. A bare token it
  recognises (OpenAI, Anthropic, GitHub, Hugging Face, AWS, Slack, Google, Stripe, JWT) is stored as `pasted-1`.
- **A guard for Claude's Bash**: commands that would print a secret (`keyfob get`, `env`, `echo $TOKEN`, reading
  the store), carry a literal token, or need a group's secrets without `keyfob run` are refused with the fix.
  It splits the command like a shell does, so a word inside a quoted grep pattern is never taken for a command.
- **Declared by plugins**: a plugin lists what it needs in a `keyfob.json`; keyfob finds it, `/keyfob` shows what
  is missing and where to get it, `keyfob check --live` asks each service whether its secrets still work.

One static binary (Rust) for macOS (arm64, x86_64) and Linux (x86_64, aarch64, musl).

## Install

As a Claude Code plugin (the binary comes with it, fetched once and checked against the release's SHA-256):

```
/plugin marketplace add yushiran/keyfob
/plugin install keyfob@keyfob
```

On its own, for a server or a script: download `keyfob-<target>.tar.gz` from
[Releases](https://github.com/yushiran/keyfob/releases) and check it against its `.sha256`, or
`cargo install --git https://github.com/yushiran/keyfob`.

## Use

```sh
keyfob add openai-key --env OPENAI_API_KEY      # asks without echo; or paste `keyfob: openai-key <token>` in Claude Code
keyfob run openai-key -- python3 eval.py        # the script reads os.environ["OPENAI_API_KEY"]
keyfob run binance -- binance-cli spot ping     # a group a plugin declared: several variables at once
keyfob ls                                       # names, variables, who needs them; never values
keyfob check --live                             # complete? still accepted by the service?
keyfob doctor                                   # storage in use; plaintext secrets left in rc files or Claude settings
```

| Command | Does |
|---|---|
| `add <name> [--env VAR] [--note T] [--from-env VAR \| --stdin]` | store; asks on the terminal by default |
| `run <group\|name\|name=VAR>... -- <cmd>` | run `cmd` with those secrets in its environment (`env` is an alias) |
| `ls`, `check [--live]`, `which VAR`, `decls`, `doctor` | look, never show values; `--json` on each |
| `get <name>` | print a value: your terminal, your programs (the guard refuses it in Claude's Bash) |
| `rm <name>`, `rename <old> <new>` | the guard asks you before an `rm` Claude runs |
| `migrate --keychain-prefix <old/>` | copy Keychain items another tool wrote as `<old/><name>` |
| `capture`, `scrub`, `patterns`, `guard` | used by the Claude Code plugin |

Storage: `KEYFOB_BACKEND` or `"backend"` in `~/.config/keyfob/config.json` (`keychain`, `secret-service`, `file`,
`auto`). On macOS keyfob goes through Apple's `/usr/bin/security`, so the Keychain's access list stays the same
across keyfob upgrades (no "allow access" dialog after each) and values travel on pipes, never in an argv.

## For plugin authors: keyfob.json

Put it at your plugin's root. keyfob reads it from every enabled plugin; people add their own in
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

Your scripts read the variables; your docs say `keyfob run scholar -- <command>`. An MCP server that needs a
key is started as `keyfob run <name> -- <server command>` in its MCP config.

## What it protects, and what it does not

- It keeps tokens out of rc files, settings files, the environment of every process, the conversation with the
  model, and (for pasted tokens and Bash commands) the transcript Claude Code writes to disk.
- The guard is a seatbelt against mistakes, not a sandbox: a command written to get around it can. It fails open
  if it cannot read its input.
- Whatever runs as you can read what you can read: on a server the file store protects against other users, not
  against root; on macOS the Keychain asks before another app reads an item.
- A pasted token is stored and replaced before the message enters the session; Claude Code's own prompt history
  file (`~/.claude/history.jsonl`) is written by the terminal, and is not covered unless that turns out otherwise
  on your version: check with a dummy token, and use `/keyfob` (its input never becomes a message) when it matters.

## Develop

`cargo test` (add `KEYFOB_TEST_KEYCHAIN=1` on macOS for the Keychain round trip), `claude plugin validate
plugins/keyfob`, `claude plugin test plugins/keyfob`. In a checkout the launcher uses `target/release` or
`target/debug`. Releases: bump `version` in `Cargo.toml`, `plugin.json` and `VERSION=` in
`plugins/keyfob/bin/keyfob`, then push a `v<version>` tag.

MIT licensed. Chinese: [README.zh-CN.md](README.zh-CN.md).
