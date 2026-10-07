// keyfob mod.
//   paste capture   a message holding `keyfob: <name> <token> [ENV_VAR]` lines or a recognisable token is
//                   stored by `keyfob capture` before it enters the conversation; the model and the transcript
//                   get the message with each value replaced, plus a note of what was stored. If storing
//                   fails the message is not sent at all, so a token never slips through.
//   /keyfob         a panel: what is needed (Claude's requests first, then what plugins declare) and what is
//                   stored; a masked field stores a value without it ever becoming a message.
//   keyfob_status   a tool for the model: names, groups, completeness; never a value.
//   keyfob_request  a tool for the model: asks for a secret by name; the panel opens on it, and once the
//                   person stores it (or puts it off) a prompt from keyfob tells the model to go on.
//                   A hook has 10 s of its own time, so the tool returns at once instead of waiting.

const PANE = 'keyfob'
const LINE = /^[ \t]*keyfob:[ \t]*([a-z0-9][a-z0-9._-]{0,63})[ \t]+(\S+)/m
const NAME_RE = /^[a-z0-9][a-z0-9._-]{0,63}$/
const ENV_RE = /^[A-Za-z_][A-Za-z0-9_]*$/
const BULLET = '•'
// theme keys, so the panel follows the person's light or dark theme
const C = { accent: 'claude', needed: 'warning', ok: 'success', bad: 'error', dim: 'subtle', frame: 'promptBorder' }
let tokens = []
let report = null
let busy = ''
let lastError = ''
let adding = null          // { name, env, why, isNew } the masked field is open for
let secret = ''            // what the masked field really holds; wiped after every store, later and close
let naming = false         // the [+ new] name field is showing
const asked = new Map()    // name -> { env } asked for by Claude this session; told back once settled

async function bin($) {
  const root = typeof $.plugin.root === 'function' ? await $.plugin.root() : $.plugin.root
  return root + '/bin/keyfob'
}

async function kf($, args, opts) {
  return $.process.run([await bin($)].concat(args), Object.assign({ timeoutMs: 60000 }, opts || {}))
}

async function json($, args, timeoutMs) {
  const r = await kf($, args, { timeoutMs: timeoutMs || 60000 })
  if (!r.stdout.trim()) throw new Error((r.stderr || '').trim().slice(0, 200) || 'exit ' + r.exitCode)
  return JSON.parse(r.stdout)
}

async function loadPatterns($) {
  try {
    const p = await json($, ['patterns', '--json'])
    tokens = (p.tokens || []).map((t) => { try { return new RegExp(t.regex) } catch (err) { return null } }).filter(Boolean)
  } catch (err) { tokens = [] }
}

async function refresh($, live) {
  if (busy) return
  busy = live ? 'checking with each service…' : 'reading…'
  $.ui.invalidate('ui.render')
  try {
    const [check, ls] = await Promise.all([json($, ['check', '--json'].concat(live ? ['--live'] : []), live ? 120000 : 60000), json($, ['ls', '--json'])])
    report = { check, ls, live: !!live, at: Date.now() }
    lastError = ''
  } catch (err) {
    lastError = String(err && err.message ? err.message : err)
  }
  busy = ''
  $.ui.invalidate('ui.render')
}

function captureNotes(c) {
  const notes = []
  for (const s of c.stored || []) notes.push('keyfob stored the secret ' + s.name + ' (' + s.length + ' characters); a command gets it as $' + s.env + ' when run as `keyfob run ' + s.name + ' -- <command>`. The value is not in this conversation and you cannot read it.')
  for (const p of c.pending || []) notes.push('A token in this message was stored by keyfob as ' + p.name + ' and replaced by [keyfob:' + p.name + ']. Ask the user what it is for, then name it: `keyfob rename ' + p.name + ' <name>`.')
  for (const f of c.failed || []) notes.push('keyfob could not store ' + f.name + ' (' + f.error + '); its value was removed from the message. Ask the user to paste it again or run `keyfob add ' + f.name + '` in their terminal.')
  return notes.join('\n')
}

/**
 * The masked field shows one bullet per character it holds; from what the field now shows, work out the real
 * text: bullets kept at the start and the end stand for the characters they replaced, anything else was typed.
 */
export function nextSecret(prev, shown) {
  const s = String(shown == null ? '' : shown)
  if (!s.includes(BULLET)) return s                                 // empty, typed fresh, or pasted over
  let p = 0
  while (p < s.length && s[p] === BULLET) p++
  let q = 0
  while (q < s.length - p && s[s.length - 1 - q] === BULLET) q++
  const typed = s.slice(p, s.length - q).split(BULLET).join('')
  if (!typed) return prev.slice(0, Math.min(prev.length, s.length)) // only deleted: from the end
  if (p + q > prev.length) return prev + typed
  return prev.slice(0, p) + typed + prev.slice(prev.length - q)
}

function tell($, text) {
  // a turn of its own once the session is idle; the model reads it as from keyfob
  try { void $.prompt.submit({ text }) } catch (err) { /* the person still sees the toast */ }
}

function close($) {
  secret = ''
  adding = null
  naming = false
  $.ui.invalidate('ui.render')
}

async function storeValue($, name, env) {
  const value = secret
  secret = ''
  adding = null
  if (!value.trim()) { $.ui.invalidate('ui.render'); return }
  const r = await kf($, ['add', name, '--stdin', '--note', 'added in /keyfob'].concat(env ? ['--env', env] : []), { stdin: value })
  if (r.exitCode !== 0) {
    $.ui.toast('keyfob: ' + (r.stderr || '').trim().slice(0, 160))
  } else {
    $.ui.toast('keyfob: stored ' + name)
    const a = asked.get(name)
    if (a) {
      asked.delete(name)
      tell($, 'keyfob: ' + name + ' is now stored ($' + a.env + '). Go on with what needed it, running the command as `keyfob run ' + name + '=' + a.env + ' -- <command>`.')
    }
  }
  report = null
  refresh($, false)
}

function later($) {
  const name = adding && adding.name
  close($)
  if (name && asked.has(name)) {
    asked.delete(name)
    tell($, 'keyfob: the person chose not to add ' + name + ' now. Say what cannot run without it, and do not ask for it again this turn.')
  }
}

function result(o) {
  return { result: JSON.stringify(o) }
}

export function register(on) {
  on('session.start', async ($, e, next) => {
    // each registration on its own: a refusal must not stop the rest
    try { await $.command.register({ name: 'keyfob', description: 'API tokens: what plugins need, what is stored, add one (values never shown)' }) } catch (err) { /* panel unavailable; capture still works */ }
    try {
      await $.tool.register({
        name: 'keyfob_status',
        // old: ...; the user adds a token by pasting `keyfob: <name> <token>` into the chat.
        description: 'keyfob holds the API tokens (names only, never values). Returns the storage in use, every secret stored or declared by a plugin, each declared group and whether it is complete, and with live=true whether each service still accepts its secrets. Call it before running a command that needs an API token, or when one fails with 401/403. Run such commands as `keyfob run <group|name> -- <command>`; to get a missing one, call keyfob_request.',
        inputSchema: { type: 'object', properties: { live: { type: 'boolean', description: 'also ask each service whether its secrets still work (slower)' } } },
      })
    } catch (err) { /* tool unavailable */ }
    try {
      await $.tool.register({
        name: 'keyfob_request',
        description: 'Ask the person for an API key, token or password without it entering the conversation. Opens the /keyfob panel on that secret with a masked field; returns at once. If it answers `opened`, say in one line what to paste, then end your turn: when the person stores it (or puts it off) a prompt from keyfob says so and you go on. `stored` means it is already there: run the command as `keyfob run <name>=<VAR> -- <command>`. Never ask for a secret in chat. Name it `<service>-<kind>` (openreview-password, github-token, openalex-key) and reuse a name keyfob_status already lists.',
        inputSchema: {
          type: 'object',
          required: ['secret', 'reason'],
          properties: {
            secret: { type: 'string', description: 'the secret\'s name: lowercase, `<service>-<kind>`' },
            env: { type: 'string', description: 'the environment variable a command reads it from (default: the name upper-cased)' },
            reason: { type: 'string', description: 'one line the person sees: what it is for' },
            obtain: { type: 'string', description: 'where to get one: a URL or a sentence' },
          },
        },
      })
    } catch (err) { /* tool unavailable */ }
    loadPatterns($)
    json($, ['doctor', '--json']).then((d) => {
      const n = (d.plaintext || []).length
      if (n) $.ui.toast('keyfob: ' + n + ' plaintext secret' + (n > 1 ? 's' : '') + ' in shell or Claude settings; /keyfob')
    }).catch(() => {})
    return next(e)
  })

  on('prompt.submit', async ($, e, next) => {
    const text = e.text || ''
    if (!LINE.test(text) && !tokens.some((r) => r.test(text))) return next(e)
    let c
    try {
      const r = await kf($, ['capture'], { stdin: text })
      if (r.exitCode !== 0) throw new Error((r.stderr || '').trim().slice(0, 200) || 'exit ' + r.exitCode)
      c = JSON.parse(r.stdout)
    } catch (err) {
      return { drop: 'keyfob could not store the token in this message, so the message was not sent: ' + String(err && err.message ? err.message : err) }
    }
    const names = (c.stored || []).map((s) => s.name).concat((c.pending || []).map((p) => p.name))
    if (names.length) $.ui.toast('keyfob: stored ' + names.join(', '))
    report = null
    return next({ ...e, text: c.text, context: [...(e.context || []), captureNotes(c)] })
  }).catch(($, e, next) => next.called ? next(e) : { drop: 'keyfob failed while checking this message for tokens, so it was not sent. Send it again, or store the token with /keyfob.' })

  on('command.run', { command: 'keyfob' }, async ($) => {
    await $.ui.open({ id: PANE, title: 'keyfob', focus: true })
    refresh($, false)
    return {}
  })

  on('ui.close', async ($, e, next) => {
    if (e.id === PANE) { secret = ''; adding = null; naming = false }
    return next(e)
  }).catch(($, e, next) => (next.called ? undefined : next(e)))   // a failure here must never keep a pane open

  on('tool.call', { tool: 'mcp__keyfob__keyfob_status' }, async ($, e) => {
    try {
      const [check, ls] = await Promise.all([json($, ['check', '--json'].concat(e.live ? ['--live'] : []), e.live ? 120000 : 60000), json($, ['ls', '--json'])])
      return { result: JSON.stringify({ storage: ls.backend, secrets: (ls.secrets || []).map((s) => ({ name: s.name, stored: s.stored, env: s.env, declared_by: s.declared_by, obtain: s.obtain })), groups: check.groups, missing: check.missing, live: check.live, problems: check.problems, how: 'keyfob run <group|name> -- <command>; for a missing one call keyfob_request (the person may also open /keyfob)' }) }
    } catch (err) {
      return { result: JSON.stringify({ error: String(err && err.message ? err.message : err) }) }
    }
  }).catch(($, e, next) => next.called ? next(e) : { result: JSON.stringify({ error: 'keyfob_status failed; run `keyfob check` in Bash' }) })

  on('tool.call', { tool: 'mcp__keyfob__keyfob_request' }, async ($, e) => {
    const name = String(e.secret || '').trim()
    const want = String(e.env || '').trim()
    const reason = String(e.reason || '').split(/\s+/).join(' ').trim()
    const obtain = String(e.obtain || '').trim()
    if (!NAME_RE.test(name)) return result({ error: 'secret: lowercase letters, digits, . _ - (e.g. openreview-password)' })
    if (want && !ENV_RE.test(want)) return result({ error: 'env: an environment variable name such as OPENREVIEW_PASSWORD' })
    if (!reason) return result({ error: 'reason: one line the person sees, saying what it is for' })
    const ls = await json($, ['ls', '--json'])
    const row = (ls.secrets || []).find((s) => s.name === name)
    if (row && row.stored) return result({ status: 'stored', env: row.env, run: 'keyfob run ' + name + '=' + row.env + ' -- <command>' })
    const req = await json($, ['request', name, '--reason', reason, '--json'].concat(want ? ['--env', want] : []).concat(obtain ? ['--obtain', obtain] : []))
    const env = req.env || want
    asked.set(name, { env })
    naming = false
    secret = ''
    adding = { name, env, why: req.declared_by && req.declared_by !== 'requested' ? 'for ' + req.declared_by : 'asked by Claude: ' + reason }
    report = null
    const opened = await $.ui.open({ id: PANE, title: 'keyfob', focus: true })
    refresh($, false)
    if (!opened || !opened.isPlaced) {
      $.ui.toast('keyfob: Claude asks for ' + name + '; type /keyfob to add it')
      return result({ status: 'waiting', env, tell_user: 'Type /keyfob, paste the value for ' + name + ' into its field and press store (the terminal is too narrow to open the panel by itself).' })
    }
    return result({ status: 'opened', env, tell_user: 'The /keyfob panel is open on ' + name + ': paste the value into the field and press store; it never becomes a message.', then: 'end your turn; a prompt from keyfob says when it is stored' })
  }).catch(($, e, next) => next.called ? next(e) : result({ error: 'keyfob_request failed; ask the person to open /keyfob and add it there' }))

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text, Button, Input } = $.ui.resolve(e)
    const T = (s, props) => Text(Object.assign({ children: [String(s)], wrap: 'truncate-end' }, props || {}))
    if (!report && !busy && !lastError) refresh($, false)
    const secrets = report ? report.ls.secrets || [] : []
    const rank = (s) => (adding && s.name === adding.name ? 0 : s.declared_by === 'requested' ? 1 : 2)
    const needed = secrets.filter((s) => !s.stored).sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
    const stored = secrets.filter((s) => s.stored).sort((a, b) => a.name.localeCompare(b.name))
    const why = (s) => (s.declared_by === 'requested' ? 'asked by Claude: ' + (s.description || '') : s.declared_by ? 'for ' + s.declared_by + (s.description ? ' · ' + s.description : '') : s.note || '')

    const field = (name, env) => Box({ key: 'field-' + name, flexDirection: 'column', marginTop: 1, children: [
      Box({ gap: 1, children: [
        Input({
          key: 'value', label: 'value: ', placeholder: 'paste it here, then Enter', submitLabel: 'store', autoFocus: true,
          value: BULLET.repeat(secret.length),
          onInput: (v) => { secret = nextSecret(secret, v); $.ui.invalidate('ui.render') },
          onSubmit: (v) => { if (v && !String(v).includes(BULLET)) secret = String(v); storeValue($, name, env) },
        }),
      ] }),
      Box({ gap: 1, children: [
        Button({ key: 'store', label: 'store', variant: 'primary', onPress: () => storeValue($, name, env) }),
        Button({ key: 'later', label: 'later', onPress: () => later($) }),
        T(secret.length ? secret.length + ' characters, shown as ' + BULLET : 'nothing typed yet', { color: C.dim }),
      ] }),
    ] })

    const row = (s, isStored) => {
      const open = adding && adding.name === s.name
      const kids = [
        Box({ key: 'line-' + s.name, gap: 1, children: [
          T((isStored ? '✓ ' : '● ') + s.name, { bold: open, color: isStored ? C.ok : C.needed }),
          T('$' + s.env, { color: C.dim }),
          Box({ flexGrow: 1, children: [] }),
          open ? null : Button({ key: (isStored ? 'replace-' : 'add-') + s.name, label: isStored ? 'replace' : 'add', onPress: () => { adding = { name: s.name, env: s.env }; secret = ''; naming = false; $.ui.invalidate('ui.render') } }),
        ].filter(Boolean) }),
      ]
      const w = why(s)
      if (w) kids.push(T('  ' + w, { color: C.dim }))
      if (!isStored && s.obtain) kids.push(T('  get one at ' + s.obtain, { color: C.dim }))
      if (open) kids.push(field(s.name, s.env))
      return Box({ key: 'row-' + s.name, flexDirection: 'column', marginBottom: 1, children: kids })
    }

    const card = (key, title, color, children) => Box({
      key, flexDirection: 'column', borderStyle: 'round', borderColor: color, paddingX: 1, marginTop: 1,
      children: [T(title, { bold: true, color })].concat(children),
    })

    const rows = []
    rows.push(Box({ key: 'head', gap: 1, children: [
      T('🔑 keyfob', { bold: true, color: C.accent }),
      T(report ? report.ls.backend + ' · ' + stored.length + ' stored · ' + needed.length + ' needed' + (report.live ? ' · live check ran' : '') : '', { color: C.dim }),
    ] }))
    if (busy) rows.push(T('⟳ ' + busy, { color: C.needed }))
    if (lastError) rows.push(T('✗ ' + lastError, { color: C.bad }))

    // a request for a name no row has yet (ls not read since): its field still opens
    if (adding && !secrets.some((s) => s.name === adding.name)) {
      rows.push(card('card-new', adding.isNew ? 'New secret' : 'Needed', C.needed, [
        T('● ' + adding.name + '   $' + (adding.env || ''), { bold: true, color: C.needed }),
        adding.why ? T('  ' + adding.why, { color: C.dim }) : null,
        field(adding.name, adding.env),
      ].filter(Boolean)))
    }
    if (needed.length) rows.push(card('card-needed', 'Needed', C.needed, needed.map((s) => row(s, false))))
    if (stored.length) rows.push(card('card-stored', 'Stored', C.frame, stored.map((s) => row(s, true))))
    if (report && !secrets.length && !adding) rows.push(T('Nothing stored or declared yet. Claude asks with keyfob_request; plugins declare theirs in keyfob.json.', { color: C.dim }))

    const groups = report ? Object.entries(report.check.groups || {}) : []
    if (groups.length) {
      rows.push(card('card-groups', 'Groups', C.frame, groups.map(([n, g]) => T(
        (g.complete ? '✓ ' : '✗ ') + n + (g.complete ? '' : '   needs ' + (g.missing || []).join(', ')) + (g.optional_missing && g.optional_missing.length ? '   (optional, absent: ' + g.optional_missing.join(', ') + ')' : ''),
        { color: g.complete ? C.ok : C.bad }))))
    }

    if (naming) {
      rows.push(Box({ key: 'naming', marginTop: 1, children: [Input({
        key: 'name', label: 'new secret name: ', placeholder: 'e.g. github-token, then Enter', submitLabel: 'next', autoFocus: true,
        onSubmit: (v) => {
          const n = String(v || '').trim()
          if (!NAME_RE.test(n)) { $.ui.toast('keyfob: lowercase letters, digits, . _ -'); return }
          naming = false
          const known = secrets.find((s) => s.name === n)
          adding = { name: n, env: known ? known.env : n.toUpperCase().replace(/[^A-Z0-9]/g, '_'), isNew: !known }
          secret = ''
          $.ui.invalidate('ui.render')
        },
      })] }))
    }

    rows.push(Box({ key: 'bar', gap: 1, marginTop: 1, children: [
      Button({ key: 'new', label: '+ new', onPress: () => { naming = true; adding = null; secret = ''; $.ui.invalidate('ui.render') } }),
      Button({ key: 'refresh', label: 'refresh', onPress: () => refresh($, false) }),
      Button({ key: 'live', label: 'live check', onPress: () => refresh($, true) }),
      Button({ key: 'close', label: 'close', onPress: () => { close($); $.ui.close({ id: PANE }) } }),
    ] }))
    rows.push(T('Values never leave this panel · keyfob run <name> -- <command>', { color: C.dim }))
    return Box({ flexDirection: 'column', children: rows })
  })
}
