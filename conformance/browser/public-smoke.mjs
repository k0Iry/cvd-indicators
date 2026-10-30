// Real-Chromium smoke: the public .wasm artifacts through the ACTUAL cvd-core
// browser host (no fabricated output). Loopback only; asserts every step and
// compares every numeric value bit-for-bit with the corpus expectations
// (computed by the same .wasm under the Rust harness, which the temporary
// native-oracle test ties to the retired native runner).
//   CVD_WASM_DIR       dir with cvd_indicator_*.wasm (required)
//   CVD_WASM_HOST_DIR  dir with host-bridge.js, plugin-worker.js, cvd_browser_host.wasm (required)
//   CVD_CORPUS         corpus.json from `cargo run -p cvd-indicators-conformance --bin corpus` (required)
//   CVD_CHROMIUM       default /usr/bin/chromium
//   CVD_SMOKE_FAULT=kill-browser  controlled failure self-test: kill the owned
//                      browser mid-run; expects bounded rejection + cleanup
// Every CDP call is bounded (timeout, and rejected on socket close/error); the
// owned Chromium process (only the one we spawned) must exit before its
// temporary profile is removed.
import http from 'node:http'
import fs from 'node:fs'
import path from 'node:path'
import os from 'node:os'
import { spawn } from 'node:child_process'
import assert from 'node:assert/strict'
import { teardown } from './harness-teardown.mjs'

const HERE = path.dirname(new URL(import.meta.url).pathname)
const { CVD_WASM_DIR: WASM, CVD_WASM_HOST_DIR: HOST, CVD_CORPUS: CORPUS } = process.env
assert.ok(WASM && HOST && CORPUS, 'CVD_WASM_DIR, CVD_WASM_HOST_DIR and CVD_CORPUS are required')
const corpus = JSON.parse(fs.readFileSync(CORPUS, 'utf8'))
const allowed = new Set(corpus.cases.map(c => c.wasm))
const types = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json', '.html': 'text/html' }
const routes = (u) => {
  if (u === '/smoke.html') return path.join(HERE, 'smoke.html')
  if (u === '/corpus.json') return CORPUS
  if (u.startsWith('/host/')) return ['host-bridge.js', 'plugin-worker.js', 'cvd_browser_host.wasm'].includes(path.basename(u)) ? path.join(HOST, path.basename(u)) : null
  if (u.startsWith('/artifacts/')) return allowed.has(path.basename(u)) ? path.join(WASM, path.basename(u)) : null
  return null
}
const external = []
const server = http.createServer((req, res) => {
  const f = routes(req.url.split('?')[0])
  if (!f || !fs.existsSync(f)) { res.writeHead(404); return res.end() }
  res.writeHead(200, { 'content-type': types[path.extname(f)] || 'application/octet-stream', 'cache-control': 'no-store' })
  fs.createReadStream(f).pipe(res)
})
const sockets = new Set()
server.on('connection', s => { sockets.add(s); s.on('close', () => sockets.delete(s)) })
await new Promise(r => server.listen(0, '127.0.0.1', r))
const port = server.address().port
const profile = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'cvd-public-smoke-'))
const FAULT = process.env.CVD_SMOKE_FAULT || ''
const RPC_TIMEOUT_MS = 30000
const chrome = spawn(process.env.CVD_CHROMIUM || '/usr/bin/chromium', ['--headless=new', '--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu',
  '--no-first-run', '--remote-debugging-port=0', '--user-data-dir=' + profile, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] })
const chromeExited = new Promise(r => { if (chrome.exitCode !== null || chrome.signalCode !== null) r(); else chrome.once('exit', () => r()) })
const withTimeout = (p, ms, what) => {
  let t; return Promise.race([p, new Promise((_, rej) => { t = setTimeout(() => rej(new Error(`${what} timed out after ${ms} ms`)), ms) })]).finally(() => clearTimeout(t))
}
// Owned teardown (main pid + exact-profile helpers, server/sockets, exact
// profile removal with reappearance check). Failure is never swallowed.
let cleanup
async function stopChrome () {
  cleanup = await teardown({ child: chrome, server, sockets, profileDir: profile, opts: { graceMs: 5000, killMs: 5000 } })
}
let result
let ws
const t0 = Date.now()
try {
  const wsUrl = await new Promise((resolve, reject) => {
    let buf = ''; const t = setTimeout(() => reject(new Error('chromium did not start')), 20000)
    chrome.once('exit', () => { clearTimeout(t); reject(new Error('chromium exited during startup')) })
    chrome.stderr.on('data', d => { buf += d; const m = buf.match(/DevTools listening on (ws:\/\/\S+)/); if (m) { clearTimeout(t); resolve(m[1]) } })
  })
  const targets = await withTimeout(fetch(`http://${new URL(wsUrl).host}/json/list`).then(r => r.json()), 10000, '/json/list')
  ws = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl)
  await withTimeout(new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', () => reject(new Error('cdp socket error')), { once: true })
  }), 10000, 'cdp open')
  let seq = 0; const pend = new Map(); let closed = null
  const failAll = (err) => { closed = err; for (const [, p] of pend) p.reject(err); pend.clear() }
  ws.addEventListener('close', () => failAll(new Error('cdp socket closed')))
  ws.addEventListener('error', () => failAll(new Error('cdp socket error')))
  chrome.once('exit', () => failAll(new Error('chromium exited')))
  ws.addEventListener('message', ev => {
    const m = JSON.parse(ev.data)
    if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id); m.error ? p.reject(new Error(`cdp ${m.error.message}`)) : p.resolve(m) }
    if (m.method === 'Network.requestWillBeSent') {
      const u = new URL(m.params.request.url)
      if (u.protocol.startsWith('http') && u.host !== `127.0.0.1:${port}`) external.push(u.href)
    }
  })
  const cdp = (method, params = {}) => {
    if (closed) return Promise.reject(closed)
    const id = ++seq
    const p = new Promise((resolve, reject) => pend.set(id, { resolve, reject }))
    ws.send(JSON.stringify({ id, method, params }))
    return withTimeout(p, RPC_TIMEOUT_MS, `cdp ${method}`).finally(() => pend.delete(id))
  }
  await cdp('Network.enable'); await cdp('Runtime.enable')
  await cdp('Page.navigate', { url: `http://127.0.0.1:${port}/smoke.html` })
  if (FAULT === 'kill-browser') chrome.kill('SIGKILL') // owned pid only
  const deadline = Date.now() + 240000
  while (Date.now() < deadline) {
    const v = await cdp('Runtime.evaluate', { expression: 'JSON.stringify(window.__smoke || null)', returnByValue: true })
    const s = v.result && v.result.result && v.result.result.value
    if (s && s !== 'null') { result = JSON.parse(s); break }
    await new Promise(r => setTimeout(r, 250))
  }
} catch (e) {
  if (FAULT !== 'kill-browser') throw e
  result = { faultError: String(e.message || e) }
} finally {
  try { ws && ws.close() } catch {}
  await stopChrome()
}
assert.ok(cleanup && cleanup.ok, 'owned cleanup ' + JSON.stringify(cleanup))
if (FAULT === 'kill-browser') {
  const ms = Date.now() - t0
  assert.ok(result && result.faultError, 'killed browser must reject the pending run')
  assert.ok(ms < RPC_TIMEOUT_MS + 20000, `settled within bound (${ms} ms)`)
  assert.ok(chrome.exitCode !== null || chrome.signalCode !== null, 'owned chromium exited')
  assert.ok(!fs.existsSync(profile), 'owned profile removed after exit')
  console.log(`HARNESS FAULT PASS: early browser death -> "${result.faultError}" in ${ms} ms; pid ${chrome.pid} exited (${chrome.signalCode || chrome.exitCode}); profile removed`)
  process.exit(0)
}
assert.ok(result, 'smoke page produced no result')
for (const s of result.steps) console.log(`${s.ok ? 'PASS' : 'FAIL'} ${s.name} — ${JSON.stringify(s.detail)}`)
assert.equal(result.steps.filter(s => !s.ok).length, 0, 'failed host steps')

const bits = (x) => { const b = Buffer.alloc(8); b.writeDoubleLE(x); return b.toString('hex') }
let numbers = 0; let okSteps = 0; let errSteps = 0; let textIdentical = 0
const cmp = (a, b, at) => {
  if (Array.isArray(a)) { assert.ok(Array.isArray(b), at); assert.equal(a.length, b.length, at); a.forEach((v, i) => cmp(v, b[i], `${at}[${i}]`)); return }
  if (a && typeof a === 'object') { assert.deepEqual(Object.keys(a).sort(), Object.keys(b).sort(), at); for (const k of Object.keys(a)) cmp(a[k], b[k], `${at}.${k}`); return }
  if (typeof a === 'number') { assert.equal(typeof b, 'number', at); assert.equal(bits(a), bits(b), at); numbers++; return }
  assert.equal(a, b, at)
}
// Host admission mirrors the SDK and may reject before the plugin runs.
const HOST_CODE = { stale_generation: 'stale_revision', sequence_gap: 'sequence_gap', invalid_input: 'invalid_input' }
assert.equal(result.cases.length, corpus.cases.length)
corpus.cases.forEach((c, ci) => {
  const got = result.cases[ci]
  assert.equal(got.name, c.name)
  if (c.create.status !== 0) {
    const code = JSON.parse(c.create.body).code
    assert.equal(got.create.ok, false, `${c.name}: load must fail`)
    assert.equal(got.create.code, 'plugin_error', c.name)
    assert.ok(got.create.message.startsWith(code), `${c.name}: ${got.create.message}`)
    errSteps++
    return
  }
  assert.ok(got.create.ok, `${c.name}: load ${got.create.code} ${got.create.message}`)
  assert.equal(got.create.descriptor, c.descriptor.length ? JSON.stringify(JSON.parse(c.descriptor)) : '', `${c.name}: descriptor`)
  c.steps.forEach((s, i) => {
    const g = got.steps[i]; const at = `${c.name} ${s.label}`
    if (s.expect.status === 0) {
      assert.ok(g.ok, `${at}: ${g.code} ${g.message}`)
      cmp(JSON.parse(s.expect.body), JSON.parse(g.text), at)
      if (g.text === s.expect.body) textIdentical++
      okSteps++
    } else {
      const code = JSON.parse(s.expect.body).code
      assert.ok(!g.ok, `${at}: expected ${code}`)
      assert.ok(g.code === HOST_CODE[code] || (g.code === 'plugin_error' && g.message.startsWith(code)), `${at}: ${g.code} ${g.message} vs ${code}`)
      errSteps++
    }
  })
  assert.equal(got.disabled, 'stale', `${c.name}: compute after disable`)
  assert.equal(got.reloaded, got.steps[0].text, `${c.name}: reload recomputes identically`)
})
assert.deepEqual(external, [], 'no non-loopback network requests')
console.log(`PUBLIC BROWSER SMOKE PASS: ${corpus.cases.length} cases, ${okSteps} outputs (${textIdentical} byte-identical text), ${numbers} numeric values bit-identical, ${errSteps} rejections matched, 0 external requests`)
