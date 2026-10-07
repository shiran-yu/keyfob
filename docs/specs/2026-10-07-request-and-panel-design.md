# keyfob 0.2.0: Claude asks for a secret, the panel opens on it

Status: design approved in chat 2026-10-07 ("开始"; "openreview和这个keyfob升级都搞好"), implemented the same
day. Two things shipped differently from the text below:

- **The tool does not wait.** A Claude Code hook has 10 s of its own time (`HookBudget.ms`), and neither a
  plain promise nor `$.clock.sleep` stops that clock, so A3 step 4's ten-minute wait cannot exist. The tool
  returns at once (`opened`, `stored`, `waiting`); when the person presses store, the mod submits a prompt
  (`$.prompt.submit`, a turn of its own once the session is idle) saying `<name> is now stored` and the run
  command; `later` submits one saying the person put it off. The effect asked for (Claude goes on without
  the person coming back to say so) is kept.
- **The tool's input names the secret `secret`**, not `name`, so it cannot collide with an event field.
  Colours are the theme's own keys (`claude`, `warning`, `success`, `error`, `subtle`, `promptBorder`), so
  the panel follows a light or dark theme, rather than fixed cyan and amber.

## Why

Storing the OpenReview credentials on 2026-10-07 showed three frictions: the person had to be told a name in
chat and then type `/keyfob`; the panel offered only a free-text name field because no plugin declares
anything yet; and a value typed into the panel's field is drawn in clear. Natural-language capture was
weighed and dropped (passwords have no shape). Notifications beyond the existing plaintext warning were
declined ("就做1就行").

## Decisions taken in chat

1. The agent asks for a secret through a tool; the tool opens `/keyfob` itself, focused on that secret.
2. The panel is redesigned to look finished: cards, colour, alignment, a masked field.
3. No new notifications (session-start, expiry, 401) and no `--expires`.
4. Plugins declare their secrets so the panel lists them by name: literature-review and bib-audit.
   paper-release is left alone: it keeps `gh` and `hf` logins in their own stores and strips `HF_TOKEN` on
   purpose for its anonymous verification (`scripts/paperrelease/remote.py:21`).
5. Migrating this machine's plaintext keys is a separate step after this ships, run with the owner's go.

## Facts from the mod API (Claude Code 2.1.293 types)

- `$.ui.open({ id, title, focus })`: a pane opened by the person's command or press is placed at any width;
  opened unasked (a model's tool call is not the person's press) it is placed from 144 columns, 110 once the
  person has opened it before; below that it waits undrawn, `{ isPlaced: false }`. `focus` is a request:
  granted while the prompt has the keys over an empty composer.
- Elements: `Box` (flex, `borderStyle` `round`…, `borderColor`, padding, gap), `Text` (colour, bold, dim,
  background, inverse, wrap/truncate), `Button` (`variant: 'primary'`, `hotkey`), `Input` (`autoFocus`,
  `value`, `onInput`, `onSubmit`). `Input` has no mask. The pane gives `bodyColumns` and `placement`
  (`dock` | `inline`).
- A model tool is `$.tool.register` + a `tool.call` hook; the hook may await before it returns.

## A. keyfob (Rust CLI and the mod)

### A1. `keyfob request <name> [--env VAR] --reason TEXT [--obtain URL] [--json]`

Records that a secret is wanted, in the person's own declaration file
`~/.config/keyfob/declarations.d/requested.json` (`{"name": "requested", "secrets": {...}}`, 0600, folder
0700), merging: other entries stay, this one is replaced. `description` holds the reason. Refuses a bad
name or variable. Prints `requested <name> ($VAR)`, or with `--json` `{name, env, stored, declared_by}`. A
secret another declaration already names is not written again (that one's env and obtain win); the command
says so and exits 0. Nothing about a value passes through it.

### A2. Optional members of a group

A group may list `"optional": ["ENV_VAR", ...]` (names of its own `env` keys). `keyfob run <group>` passes
an optional member when it is stored and skips it silently when not; only required members can make it
fail (exit 3, as today). `check` counts a group complete when its required members are stored, and adds
`optional_missing` to the JSON. Schema: the property, its description, and a refusal (a `problems` line)
for an optional name that is not one of the group's variables.

### A3. The `keyfob_request` tool (mod)

Input `{ name, env?, reason, obtain? }`. The handler:
1. `ls --json`: stored already → returns `{ status: "stored", run: "keyfob run <name>=<VAR> -- <cmd>" }`.
2. Not declared → `keyfob request …`. Declared by a plugin → no file write.
3. Sets the panel's focus target to that secret and `$.ui.open({ id: 'keyfob', title: 'keyfob', focus: true })`.
   `isPlaced: false` → a toast ("keyfob: Claude asks for <name>; type /keyfob to add it") and an immediate
   return `{ status: "waiting", tell_user: "type /keyfob …" }`.
4. Placed: waits, up to 10 minutes, for the person's answer in the panel: Store → `{ status: "stored" }`;
   Later → `{ status: "declined" }`; time out → `{ status: "waiting" }`. An aborted turn ends the wait.
The result never holds a value. The tool description tells the model: call this instead of asking for a
secret in chat; name it `<service>-<kind>` and reuse a declared name.

### A4. The panel

Drawn to `bodyColumns`, docked or inline, every long text truncated:
- **Header**: `🔑 keyfob` · storage (`file`, `keychain`, …) · `<n> stored` · `<m> needed`, and the live-check
  time when one ran.
- **Needed** card (round border, amber): the requested secrets first, then the declared-but-missing ones.
  Each row: `●` name, `$VAR`, who or why (`asked by Claude: <reason>` / `for <plugin>`), the obtain link
  dimmed on its own line, `[add]`. The row the tool targets opens with its field.
- **Stored** card (round border, dim): `✓` name, `$VAR`, who uses it, note, `[replace]`.
- **Groups** line: each declared group, complete or missing what (optional members never block).
- **Field**: a masked input, `value for <name>` · `••••••••` (`n` characters), `[store]` (primary,
  hotkey `s`) and `[later]`. Masking: the real text is kept in the mod; each `onInput` diff appends, deletes
  from the end, or replaces all when the field was cleared or pasted over; the field is redrawn as bullets.
  Submit stores through `keyfob add <name> --stdin --env <VAR> --note "added in /keyfob"`; the kept text is
  wiped after every submit, cancel and close.
- **Bar**: `[+ new]` (name, then the masked field) `[refresh]` `[live check]` `[close]`, and a dim footer:
  "values never leave this panel · keyfob run <name> -- <command>".
- Colours: one accent (cyan) for headers and focus, green for stored, amber for needed, red for errors,
  dim for metadata; no colour carries meaning alone (✓ ● ✗ marks stay).

### A5. Wording

`keyfob_status`'s description, the usage skill and `run`'s "missing" message say: Claude calls
`keyfob_request`; the person may also open `/keyfob` or run `keyfob add <name>`. Paste capture stays as it
is, mentioned last.

### A6. Release

`Cargo.toml`, `plugins/keyfob/.claude-plugin/plugin.json` and `VERSION=` in `plugins/keyfob/bin/keyfob` go to
0.2.0; a `v0.2.0` tag makes CI build the four binaries the launcher fetches. Until the tag is pushed and the
release exists, the installed plugin keeps 0.1.2. Local development builds with a user-space rustup
(minimal profile) on this aarch64 login node; the launcher prefers `target/release` inside a checkout.

## B. Declarations in two plugins

### literature-review `keyfob.json` (repository root)

Secrets, env and where to get each: `openalex-key` OPENALEX_API_KEY (openalex.org, free key: 10 000 requests a
day instead of 1 000), `s2-key` S2_API_KEY (semanticscholar.org API key form), `mineru-token` MINERU_TOKEN
(mineru.net/apiManage/token), `github-token` GITHUB_TOKEN (optional: repos.py falls back to `gh auth token`),
`wiley-tdm-token` WILEY_TDM_TOKEN, `elsevier-api-key` ELSEVIER_API_KEY, `elsevier-insttoken`
ELSEVIER_INSTTOKEN, `springer-key` SPRINGER_API_KEY, `openreview-username` OPENREVIEW_USERNAME,
`openreview-password` OPENREVIEW_PASSWORD, `openreview-token` OPENREVIEW_TOKEN (written by
`reviews.py --verify`, lasts about a day).

Groups: `litrev` (the eight API keys, all optional), `openreview-login` (username and password, required),
`openreview` (the token, required; live check `GET https://api2.openreview.net/notes?id=xQBRrtQM8u` with
`Authorization: Bearer ${OPENREVIEW_TOKEN}`). No `require_for`: the scripts still read `access.env`, so
nothing is refused. Guard `deny_paths`: `~/.config/litrev/access.env`, `cookies.txt`, `openreview.token`,
`openreview.pending`.

SKILL.md (rule 6 and step 5c) and `references/workflow.md`: with keyfob, `keyfob run litrev -- uv run
scripts/<script>.py …`; the names to request; `access.env` remains the fallback. Version 0.4.1.

### bib-audit `keyfob.json`

`openalex-key` and `s2-key` with the same variables (one value serves both plugins); group `bib-audit`, both
optional. SKILL.md: `keyfob run bib-audit -- python3 scripts/bib_audit.py …`.

## Testing

- Rust (`tests/cli.rs`): `request` writes, merges, refuses bad input, skips a declared name, never touches
  a value; optional members: `run` skips a missing optional and fails on a missing required, `check`
  completeness and `optional_missing`, a bad optional name is a problem line.
- Mod (`register.test.ts`, `claude plugin test plugins/keyfob`): the tool returns `stored` at once for a
  stored secret; for a missing one it runs `request`, opens the pane focused, and resolves `stored` when the
  panel submits (process.run faked) and `declined` on later; `isPlaced: false` toasts and returns
  `waiting`; the masking diff (append, backspace, paste over, clear) as a pure function; no value appears
  in any tool result or toast.
- `claude plugin validate` on both plugin folders; the two `keyfob.json` files against the schema.
- By hand after install: the person sees the panel open on a request, and the redesign at a narrow inline
  width and docked.

## Out of scope

Notifications, expiry metadata, natural-language capture, paper-release, the migration (step C), Windows.
