// keyfob mod.
//   paste capture  a message holding `keyfob: <name> <token> [ENV_VAR]` lines or a recognisable token is
//                  stored by `keyfob capture` before it enters the conversation; the model and the transcript
//                  get the message with each value replaced, plus a note of what was stored. If storing
//                  fails the message is not sent at all, so a token never slips through.
//   /keyfob        a panel: declared groups, what is missing, an input to paste a value into (it never
//                  becomes a message), a live check.
//   keyfob_status  a tool for the model: names, groups, completeness; never a value.

const PANE = 'keyfob'
const LINE = /^[ \t]*keyfob:[ \t]*([a-z0-9][a-z0-9._-]{0,63})[ \t]+(\S+)/m
let tokens = []
let report = null
let busy = ''
let adding = null
let lastError = ''

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

export function register(on) {
  on('session.start', async ($, e, next) => {
    // each registration on its own: a refusal must not stop the rest
    try { await $.command.register({ name: 'keyfob', description: 'API tokens: what plugins need, what is stored, add one (values never shown)' }) } catch (err) { /* panel unavailable; capture still works */ }
    try {
      await $.tool.register({
        name: 'keyfob_status',
        description: 'keyfob holds the API tokens (names only, never values). Returns the storage in use, every secret stored or declared by a plugin, each declared group and whether it is complete, and with live=true whether each service still accepts its secrets. Call it before running a command that needs an API token, or when one fails with 401/403. Run such commands as `keyfob run <group|name> -- <command>`; the user adds a token by pasting `keyfob: <name> <token>` into the chat.',
        inputSchema: { type: 'object', properties: { live: { type: 'boolean', description: 'also ask each service whether its secrets still work (slower)' } } },
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

  on('tool.call', { tool: 'mcp__keyfob__keyfob_status' }, async ($, e) => {
    try {
      const [check, ls] = await Promise.all([json($, ['check', '--json'].concat(e.live ? ['--live'] : []), e.live ? 120000 : 60000), json($, ['ls', '--json'])])
      return { result: JSON.stringify({ storage: ls.backend, secrets: (ls.secrets || []).map((s) => ({ name: s.name, stored: s.stored, env: s.env, declared_by: s.declared_by, obtain: s.obtain })), groups: check.groups, missing: check.missing, live: check.live, problems: check.problems, how: 'keyfob run <group|name> -- <command>; to add, the user pastes `keyfob: <name> <token>` into the chat or opens /keyfob' }) }
    } catch (err) {
      return { result: JSON.stringify({ error: String(err && err.message ? err.message : err) }) }
    }
  }).catch(($, e, next) => next.called ? next(e) : { result: JSON.stringify({ error: 'keyfob_status failed; run `keyfob check` in Bash' }) })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text, Button, Input } = $.ui.resolve(e)
    const T = (s, props) => Text(Object.assign({ children: [s] }, props || {}))
    const rows = []
    if (busy) rows.push(T('⟳ ' + busy, { color: 'yellow' }))
    if (lastError) rows.push(T('✗ ' + lastError, { color: 'red' }))
    if (report) {
      const { check, ls } = report
      rows.push(T('storage: ' + ls.backend + (report.live ? '   (live check ran)' : ''), { dimColor: true }))
      const groups = Object.entries(check.groups || {})
      rows.push(T(''))
      rows.push(T('Groups plugins declared', { bold: true }))
      if (!groups.length) rows.push(T('  none yet: a plugin declares them in its keyfob.json', { dimColor: true }))
      for (const [name, g] of groups) {
        const live = (check.live || []).find((l) => l.group === name)
        rows.push(Box({ children: [
          T('  ' + (g.complete ? '✓ ' : '✗ ') + name.padEnd(18), { color: g.complete ? 'green' : 'red' }),
          T(g.complete ? 'complete' : 'missing ' + g.missing.join(', '), { dimColor: true }),
          T(live ? '   live: ' + live.status : '', { color: live && live.status === 'ok' ? 'cyan' : 'red' }),
        ] }))
      }
      rows.push(T(''))
      rows.push(T('Secrets', { bold: true }))
      for (const s of ls.secrets || []) {
        rows.push(Box({ gap: 1, children: [
          T('  ' + (s.stored ? '✓ ' : '✗ ') + s.name.padEnd(24) + ' $' + s.env.padEnd(26), s.stored ? {} : { color: 'red' }),
          T(s.declared_by ? 'for ' + s.declared_by : (s.note || ''), { dimColor: true }),
          Button({ key: 'add-' + s.name, label: s.stored ? 'replace' : 'add', onPress: () => { adding = s.name; $.ui.invalidate('ui.render') } }),
        ] }))
        if (!s.stored && s.obtain) rows.push(T('      get one at ' + s.obtain, { dimColor: true }))
      }
    }
    rows.push(T(''))
    if (adding) {
      rows.push(Input({
        key: 'value', label: 'value for ' + adding + ': ', placeholder: 'paste, then Enter (it is stored, never sent as a message)', submitLabel: 'store', autoFocus: true,
        onSubmit: async (value) => {
          const name = adding
          adding = null
          if (!value || !value.trim()) { $.ui.invalidate('ui.render'); return }
          const r = await kf($, ['add', name, '--stdin', '--note', 'added in /keyfob'], { stdin: value })
          $.ui.toast(r.exitCode === 0 ? 'keyfob: stored ' + name : 'keyfob: ' + (r.stderr || '').trim().slice(0, 160))
          refresh($, false)
        },
      }))
      rows.push(Button({ key: 'cancel', label: 'cancel', onPress: () => { adding = null; $.ui.invalidate('ui.render') } }))
    } else {
      rows.push(Input({
        key: 'name', label: 'new secret name: ', placeholder: 'e.g. openai-key, then Enter', submitLabel: 'next',
        onSubmit: (name) => { name = (name || '').trim(); if (/^[a-z0-9][a-z0-9._-]{0,63}$/.test(name)) adding = name; else $.ui.toast('keyfob: lowercase letters, digits, . _ -'); $.ui.invalidate('ui.render') },
      }))
    }
    rows.push(T(''))
    rows.push(Box({ gap: 1, children: [
      Button({ key: 'refresh', label: 'refresh', onPress: () => refresh($, false) }),
      Button({ key: 'live', label: 'live check', onPress: () => refresh($, true) }),
      Button({ key: 'close', label: 'close', onPress: () => $.ui.close({ id: PANE }) }),
    ] }))
    rows.push(T('In chat: paste `keyfob: <name> <token>`. Commands: keyfob run <group|name> -- <command>.', { dimColor: true }))
    return Box({ flexDirection: 'column', children: rows })
  })
}
