import { test } from 'node:test'
import assert from 'node:assert/strict'
import { sha256 } from '@noble/hashes/sha256'

// S171's transport captures fetch when the client module first loads, so install one forwarding fetch
// before any test imports the client and let each test swap the handler.
let handler: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response> = async () => { throw new TypeError('no fixture') }
globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => handler(input, init)) as typeof fetch

test('phone upload hashes and resumes two independent files', async () => {
  const uploads = new Map<string, { data: number[]; total: number; sha256: string }>()
  let lost = false
  handler = async (input, init) => {
    const path = String(input)
    if (path === '/uploads') {
      const body = JSON.parse(String(init?.body))
      const id = String(uploads.size + 1)
      uploads.set(id, { data: [], total: body.byteLength, sha256: body.sha256 })
      return Response.json({ uploadId: id, received: 0, total: body.byteLength, sha256: body.sha256 })
    }
    const [, id, suffix] = path.match(/^\/uploads\/(\d+)(.*)$/)!
    const item = uploads.get(id)!
    if (suffix.startsWith('/chunks')) {
      const bytes = new Uint8Array(await (init!.body as Blob).arrayBuffer())
      assert.equal((init!.headers as Record<string, string>)['X-Momso-Chunk-Sha256'], Buffer.from(sha256(bytes)).toString('hex'))
      assert.equal(Number(new URL('https://fixture.invalid' + suffix).searchParams.get('offset')), item.data.length)
      item.data.push(...bytes)
      if (!lost) { lost = true; throw new TypeError('fixture network loss') }
    } else if (suffix === '/finish') {
      assert.equal(Buffer.from(sha256(new Uint8Array(item.data))).toString('hex'), item.sha256)
      return Response.json({ id: 'asset-' + id, originalFileName: 'fixture', mediaType: 'text/plain' })
    }
    return Response.json({ uploadId: id, received: item.data.length, total: item.total, sha256: item.sha256 })
  }
  {
    const { uploadAttachment } = await import('../src/server-client')
    const [a, b] = await Promise.all([uploadAttachment(new File(['one'], 'a.txt')), uploadAttachment(new File(['two'], 'b.txt'))])
    assert.notEqual(a.assetId, b.assetId)
    assert.equal(uploads.size, 2)
    await assert.rejects(uploadAttachment(new File(['bad'], 'wrong.txt'), undefined, '1'), /upload_file_mismatch/)
  }
})

test('phone upload waits while the server disk reservation is busy', async () => {
  let data: number[] = []
  let total = 0
  let digest = ''
  let busyChunk = true
  let busyFinish = true
  let chunkWrites = 0
  handler = async (input, init) => {
    const path = String(input)
    if (path === '/uploads') {
      const body = JSON.parse(String(init?.body))
      total = body.byteLength; digest = body.sha256
      return Response.json({ uploadId: '7', received: 0, total, sha256: digest })
    }
    if (path.startsWith('/uploads/7/chunks')) {
      if (busyChunk) { busyChunk = false; return Response.json({ error: 'reservation_busy' }, { status: 409 }) }
      chunkWrites += 1
      data.push(...new Uint8Array(await (init!.body as Blob).arrayBuffer()))
      return Response.json({ uploadId: '7', received: data.length, total })
    }
    if (path === '/uploads/7/finish') {
      if (busyFinish) { busyFinish = false; return Response.json({ error: 'reservation_busy' }, { status: 409 }) }
      assert.equal(Buffer.from(sha256(new Uint8Array(data))).toString('hex'), digest)
      return Response.json({ id: 'asset-7', originalFileName: 'busy.txt', mediaType: 'text/plain' })
    }
    return Response.json({ uploadId: '7', received: data.length, total, sha256: digest })
  }
  {
    const { uploadAttachment } = await import('../src/server-client')
    const result = await uploadAttachment(new File(['busy'], 'busy.txt'))
    assert.equal(result.assetId, 'asset-7')
    assert.equal(chunkWrites, 1)
    assert.equal(busyChunk || busyFinish, false)
  }
})
