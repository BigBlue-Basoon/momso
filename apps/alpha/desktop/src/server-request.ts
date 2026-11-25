/** Same-origin server transport. A waking gateway must never replay a write. */
export class ServerError extends Error {
  constructor(
    readonly code: string,
    readonly status: number | null,
    readonly retryable = false,
    readonly detail: unknown = null,
  ) { super(code); this.name = 'ServerError' }
}

export type ServerConnectionState = 'unknown' | 'waking' | 'ready' | 'unavailable'
export type ServerConnectionEvent = { state: ServerConnectionState; retryAfterSeconds: number | null }
const CONNECTION_EVENT = 'momso:server-connection'
let connection: ServerConnectionEvent = { state: 'unknown', retryAfterSeconds: null }

export function getServerConnectionState(): ServerConnectionEvent { return { ...connection } }
export function onServerConnectionChange(listener: (event: ServerConnectionEvent) => void): () => void {
  const handler = (event: Event) => listener((event as CustomEvent<ServerConnectionEvent>).detail)
  window.addEventListener(CONNECTION_EVENT, handler)
  return () => window.removeEventListener(CONNECTION_EVENT, handler)
}
function notifyConnection(value: ServerConnectionEvent) {
  if (connection.state === value.state && connection.retryAfterSeconds === value.retryAfterSeconds) return
  connection = value
  if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent(CONNECTION_EVENT, { detail: { ...value } }))
}

type Options = {
  fetch?: typeof fetch
  now?: () => number
  sleep?: (ms: number, signal?: AbortSignal | null) => Promise<void>
  notify?: (value: ServerConnectionEvent) => void
  signedOut?: () => void
  maxWaitMs?: number
  maxRetries?: number
}

function wait(ms: number, signal?: AbortSignal | null): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(new ServerError('cancelled', null)); return }
    const timer = setTimeout(() => { signal?.removeEventListener('abort', cancel); resolve() }, ms)
    const cancel = () => { clearTimeout(timer); signal?.removeEventListener('abort', cancel); reject(new ServerError('cancelled', null)) }
    signal?.addEventListener('abort', cancel, { once: true })
  })
}

async function jsonBody(response: Response): Promise<Record<string, unknown> | null> {
  if (!(response.headers.get('content-type') ?? '').toLowerCase().includes('json')) return null
  try {
    const value: unknown = await response.clone().json()
    return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
  } catch { return null }
}

export function isSignedOut(error: unknown): boolean {
  return error instanceof ServerError && (error.status === 401 || ['signed_out', 'login_required', 'session_expired'].includes(error.code))
}

export function createServerTransport(options: Options = {}) {
  const doFetch = options.fetch ?? fetch
  const now = options.now ?? Date.now
  const sleep = options.sleep ?? wait
  const notify = options.notify ?? notifyConnection
  const signedOut = options.signedOut ?? (() => {})
  const maxWaitMs = options.maxWaitMs ?? 180_000
  const maxRetries = options.maxRetries ?? 30
  let observedWaking = false

  async function call(path: string, init: RequestInit = {}): Promise<Response> {
    const method = (init.method ?? 'GET').toUpperCase()
    const safe = method === 'GET' || method === 'HEAD'
    const started = now()
    let retries = 0
    let wasWaking = false
    for (;;) {
      let response: Response
      const controller = new AbortController()
      const cancel = () => controller.abort()
      if (init.signal?.aborted) throw new ServerError('cancelled', null)
      init.signal?.addEventListener('abort', cancel, { once: true })
      let streaming = false
      const timer = safe ? setTimeout(cancel, Math.max(0, maxWaitMs - (now() - started))) : null
      try {
        response = await doFetch(path, { ...init, signal: controller.signal, redirect: 'manual', credentials: 'same-origin', cache: 'no-store' })
        streaming = response.ok && (response.headers.get('content-type') ?? '').includes('text/event-stream')
      } catch {
        if (init.signal?.aborted) throw new ServerError('cancelled', null)
        if (wasWaking && controller.signal.aborted) {
          observedWaking = false
          notify({ state: 'unavailable', retryAfterSeconds: null })
          throw new ServerError('waking_timeout', 503, true, { attempts: retries + 1, cause: 'waking' })
        }
        throw new ServerError('server_unreachable', null, true, { phase: 'request' })
      } finally {
        if (timer !== null) clearTimeout(timer)
        if (!streaming) init.signal?.removeEventListener('abort', cancel)
      }
      if (response.type === 'opaqueredirect') { signedOut(); throw new ServerError('signed_out', null) }
      const value = (response.status === 401 || response.status === 403 || response.status === 503)
        ? await jsonBody(response) : null
      const code = typeof value?.error === 'string' ? value.error : null
      if (response.status === 401 || response.status === 403) {
        if (response.status === 401 || (!value && response.status === 403)) signedOut()
        throw new ServerError(code ?? (response.status === 401 || !value ? 'signed_out' : 'forbidden'), response.status,
          false, value?.detail ?? null)
      }
      if (response.status === 503 && code === 'waking') {
        wasWaking = true
        observedWaking = true
        const delay = typeof value?.retryAfterSeconds === 'number' && Number.isFinite(value.retryAfterSeconds) && value.retryAfterSeconds >= 0
          ? value.retryAfterSeconds : 2
        notify({ state: 'waking', retryAfterSeconds: delay })
        if (!safe) throw new ServerError('waking', 503, true, { retryAfterSeconds: delay, replayed: false })
        const ms = delay * 1000
        if (retries >= maxRetries || now() - started + ms >= maxWaitMs) {
          observedWaking = false
          notify({ state: 'unavailable', retryAfterSeconds: null })
          throw new ServerError('waking_timeout', 503, true, { attempts: retries + 1, cause: 'waking' })
        }
        retries += 1
        try { await response.body?.cancel() } catch { /* already closed */ }
        await sleep(ms, init.signal)
        continue
      }
      if (observedWaking && response.ok) {
        observedWaking = false
        notify({ state: 'ready', retryAfterSeconds: null })
      }
      return response
    }
  }

  async function request<T>(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<T> {
    const response = await call(path, { method, headers: body === undefined ? headers : { ...headers, 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) })
    const type = response.headers.get('content-type') ?? ''
    let text: string
    try { text = await response.text() } catch { throw new ServerError('response_interrupted', response.status, true) }
    if (!type.includes('json') && /^\s*</.test(text)) {
      if (response.ok) { signedOut(); throw new ServerError('signed_out', response.status) }
      throw new ServerError(`http_${response.status}`, response.status, response.status >= 500)
    }
    let value: unknown = null
    try { value = text ? JSON.parse(text) : null } catch { value = null }
    if (!response.ok) {
      const object = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
      const code = typeof object?.error === 'string' ? object.error : `http_${response.status}`
      throw new ServerError(code, response.status, response.status >= 500, object?.detail ?? null)
    }
    return value as T
  }
  return { call, request }
}

/** One SSE connection only. Reconnection and byte offsets belong to the caller. */
export async function readServerSse(body: ReadableStream<Uint8Array>, onEvent: (name: string, payload: Record<string, unknown>) => boolean | void) {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    for (;;) {
      let result: ReadableStreamReadResult<Uint8Array>
      try { result = await reader.read() } catch { throw new ServerError('stream_interrupted', null, true, { phase: 'stream' }) }
      const { value, done } = result
      if (done) return false
      buffer += decoder.decode(value, { stream: true })
      const blocks = buffer.split(/\r?\n\r?\n/)
      buffer = blocks.pop() ?? ''
      for (const block of blocks) {
        let name = 'message'
        const data: string[] = []
        for (const line of block.split(/\r?\n/)) {
          if (line.startsWith('event:')) name = line.slice(6).trim()
          else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''))
        }
        let payload: Record<string, unknown> | null = null
        try { payload = JSON.parse(data.join('\n')) } catch { payload = null }
        if (!payload || typeof payload !== 'object') continue
        if (onEvent(name, payload) === true) { try { await reader.cancel() } catch { /* already closed */ }; return true }
      }
    }
  } finally { reader.releaseLock() }
}
