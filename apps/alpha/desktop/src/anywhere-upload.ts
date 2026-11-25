import { sha256 } from '@noble/hashes/sha256'

const CHUNK_BYTES = 8 * 1024 * 1024
const hex = (bytes: Uint8Array) => Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('')

type Progress = { uploadId: string; received: number; total: number; sha256?: string }
type Options = { uploadId?: string; fetchImpl?: typeof fetch; onProgress?: (progress: Progress) => void; sleep?: (ms: number) => Promise<void> }

const BUSY_RETRIES = 30
const wait = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms))

/**
 * The server refuses a chunk or the final copy with `reservation_busy` before
 * writing anything while another large write (for example a backup) holds the
 * disk reservation. Sending the same request again later is safe.
 */
async function retryWhileBusy<T>(sleep: (ms: number) => Promise<void>, send: () => Promise<T>): Promise<T> {
  let delay = 2000
  for (let attempt = 0; ; attempt++) {
    try {
      return await send()
    } catch (error) {
      if (!(error instanceof Error) || error.message !== 'reservation_busy' || attempt >= BUSY_RETRIES) throw error
      await sleep(delay)
      delay = Math.min(delay * 2, 30000)
    }
  }
}

async function jsonRequest<T>(fetchImpl: typeof fetch, path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetchImpl(path, { credentials: 'same-origin', ...init })
  const body = await response.json()
  if (!response.ok) throw new Error(typeof body?.error === 'string' ? body.error : 'upload_failed')
  return body as T
}

/** Uploads only through the authenticated anywhere server, never the local desktop path. */
export async function uploadAnywhereFile(file: File, options: Options = {}): Promise<unknown> {
  const request = options.fetchImpl ?? fetch
  const sleep = options.sleep ?? wait
  const hash = sha256.create()
  for (let offset = 0; offset < file.size; offset += CHUNK_BYTES) {
    hash.update(new Uint8Array(await file.slice(offset, offset + CHUNK_BYTES).arrayBuffer()))
  }
  const fileHash = hex(hash.digest())
  let progress: Progress
  if (options.uploadId) {
    progress = await jsonRequest<Progress>(request, `/uploads/${encodeURIComponent(options.uploadId)}`)
    if (progress.total !== file.size || progress.sha256 !== fileHash) throw new Error('upload_file_mismatch')
  } else {
    progress = await jsonRequest<Progress>(request, '/uploads', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ fileName: file.name, mediaType: file.type || 'application/octet-stream', byteLength: file.size, sha256: fileHash }),
    })
  }
  options.onProgress?.(progress)
  let retries = 0
  while (progress.received < progress.total) {
    const offset = progress.received
    const bytes = new Uint8Array(await file.slice(offset, offset + CHUNK_BYTES).arrayBuffer())
    const url = `/uploads/${encodeURIComponent(progress.uploadId)}/chunks?offset=${offset}`
    try {
      progress = await retryWhileBusy(sleep, () => jsonRequest<Progress>(request, url, { method: 'PUT', headers: { 'Content-Type': 'application/octet-stream', 'X-Momso-Chunk-Sha256': hex(sha256(bytes)) }, body: bytes }))
      retries = 0
    } catch (error) {
      // A lost response can follow a durable write. Resume only from a verified offset.
      if (!(error instanceof TypeError)) throw error
      const current = await jsonRequest<Progress>(request, `/uploads/${encodeURIComponent(progress.uploadId)}`)
      if (current.total !== file.size || ![offset, offset + bytes.length].includes(current.received)) throw new Error('offset_mismatch')
      if (current.received === offset && ++retries > 3) throw new Error('upload_failed')
      progress = current
    }
    options.onProgress?.(progress)
  }
  return retryWhileBusy(sleep, () => jsonRequest(request, `/uploads/${encodeURIComponent(progress.uploadId)}/finish`, { method: 'POST' }))
}
