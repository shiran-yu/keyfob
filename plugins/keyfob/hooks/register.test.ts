import { test, expect } from 'claude-code/testing'

// keyfob's paste capture: the binary is faked by answering process.run beneath the plugin, and the test's own
// prompt.submit hook stands for the session, so what reaches it is what the model and the transcript would get.

const TOKEN = 'ghp_abcdefghijklmnopqrstuvwxyz0123456789AB'

function engine(on: any, capture: (stdin: string) => { exitCode: number; stdout: string; stderr?: string }) {
  const runs: any[] = []
  const reached: any[] = []
  const toasts: string[] = []
  const answer = (exitCode: number, stdout: string, stderr = '') => ({ value: { exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false } })
  on('process.run', async ($: any, e: any) => {
    runs.push(e)
    const sub = e.argv[1]
    if (sub === 'patterns') return answer(0, JSON.stringify({ tokens: [{ secret: '', regex: '\\bgh[pousr]_[A-Za-z0-9]{36,}' }] }))
    if (sub === 'doctor') return answer(0, JSON.stringify({ plaintext: [] }))
    if (sub === 'capture') { const r = capture(e.init?.stdin ?? ''); return answer(r.exitCode, r.stdout, r.stderr ?? '') }
    return answer(0, '{}')
  })
  on('command.register', async ($: any, e: any) => ({ value: { command: e.name } }))
  on('tool.register', async ($: any, e: any) => ({ value: { tool: e.name } }))
  on('ui.toast', async ($: any, e: any) => { toasts.push(String(e.text ?? e)); return { value: undefined } })
  on('session.start', async ($: any, e: any) => ({ cwd: e.cwd }))
  on('prompt.submit', async ($: any, e: any) => { reached.push(e); return { text: e.text, context: e.context } })
  return { runs, reached, toasts }
}

async function start($: any) {
  await $.session.start({ cwd: '/tmp' })
  await new Promise((r) => setTimeout(r, 20)) // patterns load in the background
}

test('a message without a token passes untouched and keyfob is not run on it', async ($, on) => {
  const t = engine(on, () => { throw new Error('capture must not run') })
  await start($)
  await ($ as any).prompt.submit({ text: 'what is in my queue?', asUser: true })
  expect(t.reached.length).toBe(1)
  expect(t.reached[0].text).toBe('what is in my queue?')
  expect(t.runs.some((r) => r.argv[1] === 'capture')).toBe(false)
})

test('a keyfob: line is stored and only the placeholder reaches the session', async ($, on) => {
  const msg = 'here it is\nkeyfob: s2-key abcdefghijklmnop1234 S2_API_KEY\nthanks'
  const t = engine(on, (stdin) => {
    expect(stdin).toBe(msg)
    return { exitCode: 0, stdout: JSON.stringify({ text: 'here it is\nkeyfob: s2-key [stored in keyfob]\nthanks', stored: [{ name: 's2-key', env: 'S2_API_KEY', length: 20 }], pending: [], failed: [] }) }
  })
  await start($)
  await ($ as any).prompt.submit({ text: msg, asUser: true })
  expect(t.reached.length).toBe(1)
  const seen = JSON.stringify(t.reached[0])
  expect(seen.includes('abcdefghijklmnop1234')).toBe(false)
  expect(t.reached[0].text.includes('keyfob: s2-key [stored in keyfob]')).toBe(true)
  expect((t.reached[0].context ?? []).join('\n').includes('keyfob run s2-key')).toBe(true)
})

test('a loose token is stored as pasted-N and the model is told to ask for its name', async ($, on) => {
  const t = engine(on, () => ({ exitCode: 0, stdout: JSON.stringify({ text: 'use [keyfob:pasted-1] please', stored: [], pending: [{ name: 'pasted-1', length: 40, declared: false }], failed: [] }) }))
  await start($)
  await ($ as any).prompt.submit({ text: 'use ' + TOKEN + ' please', asUser: true })
  expect(JSON.stringify(t.reached).includes(TOKEN)).toBe(false)
  expect((t.reached[0].context ?? []).join('\n').includes('keyfob rename pasted-1')).toBe(true)
})

test('when keyfob cannot store, the message is not sent at all', async ($, on) => {
  const t = engine(on, () => ({ exitCode: 2, stdout: '', stderr: 'keyfob: the Keychain is locked' }))
  await start($)
  await ($ as any).prompt.submit({ text: 'keyfob: s2-key abcdefghijklmnop1234', asUser: true })
  expect(t.reached.length).toBe(0)
})

test('a broken answer from keyfob also stops the message (fails closed)', async ($, on) => {
  const t = engine(on, () => ({ exitCode: 0, stdout: 'not json' }))
  await start($)
  await ($ as any).prompt.submit({ text: 'token ' + TOKEN, asUser: true })
  expect(t.reached.length).toBe(0)
})

// ------------------------------------------------------------------ keyfob_request and the panel (0.2.0)

import { nextSecret } from './register.js'

const VALUE = 'pw-Very$ecret-123'

/** The engine beneath the plugin, with a stand-in keyfob binary that keeps its own store. */
function store(on: any, opts: { placed?: boolean; stored?: string[] } = {}) {
  const runs: any[] = [], reached: any[] = [], toasts: string[] = [], adds: any[] = [], opens: any[] = []
  const kept = new Map<string, { env: string; by: string; why: string; value?: string }>()
  for (const n of opts.stored ?? []) kept.set(n, { env: n.toUpperCase().replace(/[-.]/g, '_'), by: '', why: '', value: 'x' })
  const answer = (exitCode: number, stdout: string, stderr = '') => ({ value: { exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false } })
  on('process.run', async ($: any, e: any) => {
    runs.push(e)
    const [sub, ...rest] = e.argv.slice(1)
    if (sub === 'ls') return answer(0, JSON.stringify({ backend: 'file', secrets: [...kept].map(([name, s]) => ({ name, stored: !!s.value, env: s.env, declared_by: s.by, description: s.why, obtain: '', note: '' })) }))
    if (sub === 'check') return answer(0, JSON.stringify({ groups: {}, missing: [], live: [], problems: [] }))
    if (sub === 'request') {
      const name = rest[0]
      const env = rest.includes('--env') ? rest[rest.indexOf('--env') + 1] : name.toUpperCase().replace(/[-.]/g, '_')
      if (!kept.has(name)) kept.set(name, { env, by: 'requested', why: rest[rest.indexOf('--reason') + 1] })
      return answer(0, JSON.stringify({ name, env: kept.get(name)!.env, stored: false, declared_by: kept.get(name)!.by }))
    }
    if (sub === 'add') {
      adds.push({ argv: e.argv, stdin: e.init?.stdin })
      kept.set(rest[0], { ...(kept.get(rest[0]) ?? { env: '', by: '', why: '' }), value: e.init?.stdin })
      return answer(0, 'stored ' + rest[0])
    }
    if (sub === 'patterns') return answer(0, JSON.stringify({ tokens: [] }))
    if (sub === 'doctor') return answer(0, JSON.stringify({ plaintext: [] }))
    return answer(0, '{}')
  })
  on('command.register', async ($: any, e: any) => ({ value: { command: e.name } }))
  on('tool.register', async ($: any, e: any) => ({ value: { tool: e.name } }))
  on('ui.toast', async ($: any, e: any) => { toasts.push(String(e.text ?? e)); return { value: undefined } })
  on('ui.open', async ($: any, e: any) => { opens.push(e); return { value: opts.placed === false ? { isPlaced: false, reason: 'narrow' } : { isPlaced: true } } })
  on('session.start', async ($: any, e: any) => ({ cwd: e.cwd }))
  on('prompt.submit', async ($: any, e: any) => { reached.push(e); return { text: e.text, context: e.context } })
  return { runs, reached, toasts, adds, opens, kept }
}

async function ask($: any, input: any) {
  const r = await $.tool.call({ tool: 'mcp__keyfob__keyfob_request', ...input })
  return JSON.parse(r.text ?? r.result ?? '{}')
}

const PANE_PROPS = { title: 'keyfob', isFocused: true, bodyColumns: 72, placement: 'dock', scroll: { offset: 0, bodyRows: 40 } } as any

test('the masked field works out the real text from what the person did to it', () => {
  expect(nextSecret('', 'abc')).toBe('abc')             // typed or pasted into an empty field
  expect(nextSecret('abc', '•••d')).toBe('abcd')        // typed at the end
  expect(nextSecret('abcd', '•••')).toBe('abc')         // backspace
  expect(nextSecret('abc', 'xyz')).toBe('xyz')          // selected all and pasted over
  expect(nextSecret('abc', '••X•')).toBe('abXc')        // typed in the middle
  expect(nextSecret('abc', '')).toBe('')                // cleared
})

test('a secret already stored is reported at once and nothing opens', async ($, on) => {
  const t = store(on, { stored: ['s2-key'] })
  await start($)
  const r = await ask($, { secret: 's2-key', reason: 'search' })
  expect(r.status).toBe('stored')
  expect(r.run).toContain('keyfob run s2-key=S2_KEY')
  expect(t.opens.length).toBe(0)
  expect(t.runs.some((x: any) => x.argv[1] === 'request')).toBe(false)
})

test('a request opens the panel on the secret; storing it tells Claude to go on, and no value leaks', async ($, on) => {
  const t = store(on)
  await start($)
  for (const surface of ['terminal', 'desktop'] as const) {
    t.kept.delete('openreview-password')                 // each surface starts with it missing
    const r = await ask($, { secret: 'openreview-password', env: 'OPENREVIEW_PASSWORD', reason: 'log in to OpenReview' })
    expect(r.status).toBe('opened')
    expect(t.opens.at(-1)).toMatchObject({ id: 'keyfob', focus: true })
    const ui = await ($ as any).ui.mount({ plugin: 'keyfob', surface, component: 'Pane', props: PANE_PROPS, requestId: 'keyfob' })
    expect(await ui.find({ type: 'Text', text: /openreview-password/ })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: /asked by Claude: log in to OpenReview/ })).toBeDefined()
    await ui.input({ key: 'value', text: VALUE.slice(0, 5), kind: 'change' })
    await ui.input({ key: 'value', text: '•'.repeat(5) + VALUE.slice(5), kind: 'change' })
    expect(JSON.stringify(await ui.drawn())).not.toContain(VALUE.slice(0, 5))
    await ui.press({ key: 'store' })
    expect(t.adds.at(-1).stdin).toBe(VALUE)
    expect(t.adds.at(-1).argv.join(' ')).not.toContain(VALUE)
    expect(t.adds.at(-1).argv).toEqual(expect.arrayContaining(['add', 'openreview-password', '--stdin', '--env', 'OPENREVIEW_PASSWORD']))
    expect(t.reached.map((p: any) => p.text).join('\n')).toContain('openreview-password is now stored')
    expect(JSON.stringify(t.reached) + t.toasts.join('\n') + JSON.stringify(r) + JSON.stringify(await ui.drawn())).not.toContain(VALUE)
    await ui.unmount()
  }
})

test('later tells Claude the person did not add it', async ($, on) => {
  const t = store(on)
  await start($)
  await ask($, { secret: 'mineru-token', reason: 'convert papers' })
  const ui = await ($ as any).ui.mount({ plugin: 'keyfob', surface: 'terminal', component: 'Pane', props: PANE_PROPS, requestId: 'keyfob' })
  await ui.input({ key: 'value', text: 'half', kind: 'change' })
  await ui.press({ key: 'later' })
  expect(t.adds.length).toBe(0)
  expect(t.reached.map((p: any) => p.text).join('\n')).toContain('chose not to add mineru-token')
})

test('a terminal too narrow for the panel gets a toast and the tool says what to tell the person', async ($, on) => {
  const t = store(on, { placed: false })
  await start($)
  const r = await ask($, { secret: 'hf-token', reason: 'push weights' })
  expect(r.status).toBe('waiting')
  expect(r.tell_user).toContain('/keyfob')
  expect(t.toasts.join('\n')).toContain('hf-token')
})

test('bad input is refused before anything runs', async ($, on) => {
  const t = store(on)
  await start($)
  expect((await ask($, { secret: 'Bad Name', reason: 'x' })).error).toBeDefined()
  expect((await ask($, { secret: 'ok-name', env: '1BAD', reason: 'x' })).error).toBeDefined()
  expect((await ask($, { secret: 'ok-name', reason: '' })).error).toBeDefined()
  expect(t.runs.some((x: any) => x.argv[1] === 'request')).toBe(false)
})
