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
