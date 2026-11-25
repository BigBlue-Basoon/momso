import test from 'node:test'
import assert from 'node:assert/strict'
import { createServerTransport, readServerSse, ServerError } from '../src/server-request'

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
const waking = () => json({ error: 'waking', retryAfterSeconds: 2 }, 503)

test('waking twice then ready retries only the read and publishes connection state', async () => {
  let calls = 0, now = 0
  const states: string[] = [], delays: number[] = []
  const transport = createServerTransport({ fetch: async () => ++calls < 3 ? waking() : json({ ok: true }), now: () => now,
    sleep: async (ms) => { delays.push(ms); now += ms }, notify: (event) => states.push(event.state) })
  assert.deepEqual(await transport.request('GET', '/documents'), { ok: true })
  assert.equal(calls, 3); assert.deepEqual(delays, [2000, 2000]); assert.deepEqual(states, ['waking', 'waking', 'ready'])
})

test('waking does not replay a write', async () => {
  let calls = 0
  const transport = createServerTransport({ fetch: async () => { calls++; return waking() }, notify: () => {} })
  await assert.rejects(transport.request('POST', '/save', { text: 'test' }), (error: ServerError) => {
    assert.equal(error.code, 'waking'); assert.equal(error.retryable, true); assert.deepEqual(error.detail, { retryAfterSeconds: 2, replayed: false }); return true
  })
  assert.equal(calls, 1)
})

test('waking time limit is a structured error', async () => {
  let now = 0, calls = 0
  const transport = createServerTransport({ fetch: async () => { calls++; return waking() }, now: () => now,
    sleep: async (ms) => { now += ms }, maxWaitMs: 3000, notify: () => {} })
  await assert.rejects(transport.request('GET', '/documents'), (error: ServerError) => {
    assert.equal(error.code, 'waking_timeout'); assert.equal(error.retryable, true); assert.deepEqual(error.detail, { attempts: 2, cause: 'waking' }); return true
  })
  assert.equal(calls, 2)
})

test('authentication and server failure codes stay structured', async () => {
  for (const [status, code] of [[401, 'session_expired'], [403, 'origin_not_allowed'], [503, 'browser_memory_low'], [507, 'disk_low'], [507, 'backup_disk_low'], [500, 'restore_failed'], [503, 'proxy_not_running']] as const) {
    let signedOut = 0
    const transport = createServerTransport({ fetch: async () => json({ error: code, detail: { phase: 'test' } }, status), signedOut: () => { signedOut++ } })
    await assert.rejects(transport.request('GET', '/test'), (error: ServerError) => {
      assert.equal(error.code, code); assert.equal(error.status, status); assert.deepEqual(error.detail, { phase: 'test' }); return true
    })
    assert.equal(signedOut, status === 401 ? 1 : 0)
  }
})

test('interrupted stream reports the cause and a resumed stream delivers later events', async () => {
  const encoder = new TextEncoder(), events: string[] = []
  let reads = 0
  const broken = new ReadableStream<Uint8Array>({ pull(controller) {
    if (reads++ === 0) controller.enqueue(encoder.encode('event: chat-delta\ndata: {"delta":"first","offset":5}\n\n'))
    else controller.error(new Error('offline'))
  } })
  await assert.rejects(readServerSse(broken, (_name, payload) => { events.push(String(payload.delta)) }),
    (error: ServerError) => error.code === 'stream_interrupted' && error.retryable)
  const resumed = new ReadableStream<Uint8Array>({ start(controller) {
    controller.enqueue(encoder.encode('event: chat-delta\ndata: {"delta":"second","offset":11}\n\nevent: chat-done\ndata: {"state":"completed"}\n\n')); controller.close()
  } })
  assert.equal(await readServerSse(resumed, (name, payload) => { if (name === 'chat-done') return true; events.push(String(payload.delta)) }), true)
  assert.deepEqual(events, ['first', 'second'])
})

test('streaming request keeps its cancellation signal after response headers', async () => {
  const controller = new AbortController()
  let inner: AbortSignal | undefined
  const transport = createServerTransport({ fetch: async (_path, init) => { inner = init?.signal ?? undefined; return new Response('stream', { headers: { 'content-type': 'text/event-stream' } }) } })
  await transport.call('/events', { signal: controller.signal }); controller.abort()
  assert.equal(inner?.aborted, true)
})

test('chat request resumes a broken stream from its byte offset without reposting', async () => {
  const oldFetch = globalThis.fetch
  const requests: string[] = [], chunks: string[] = []
  let posts = 0
  const encoder = new TextEncoder()
  globalThis.fetch = async (input, init) => {
    const path = String(input); requests.push(path)
    if (init?.method === 'POST') { posts++; return json({ turnId: 'turn', userMessageId: 'message' }) }
    if (path.endsWith('after=0')) {
      let reads = 0
      return new Response(new ReadableStream<Uint8Array>({ pull(controller) {
        if (reads++ === 0) controller.enqueue(encoder.encode('event: chat-delta\ndata: {"delta":"abc","offset":3}\n\n'))
        else controller.error(new Error('connection lost'))
      } }), { headers: { 'content-type': 'text/event-stream' } })
    }
    if (path.endsWith('after=3')) return new Response('event: chat-delta\ndata: {"delta":"def","offset":6}\n\nevent: chat-done\ndata: {"state":"completed"}\n\n', { headers: { 'content-type': 'text/event-stream' } })
    return json({ inputs: [], events: [] })
  }
  try {
    const { networkInvoke, networkListen } = await import('../src/server-client')
    const done = new Promise<void>(async (resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('turn did not complete')), 5000)
      await networkListen<{ delta: string }>('chat-stream', (event) => chunks.push(event.payload.delta))
      await networkListen('chat-done', () => { clearTimeout(timeout); resolve() })
    })
    await networkInvoke('chat_send', { chatId: 'chat', text: 'test' })
    await done
    assert.equal(posts, 1); assert.deepEqual(chunks, ['abc', 'def'])
    assert.ok(requests.some((path) => path.endsWith('after=3')))
  } finally { globalThis.fetch = oldFetch }
})
