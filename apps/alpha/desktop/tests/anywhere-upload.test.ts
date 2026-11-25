import { test } from 'node:test'
import assert from 'node:assert/strict'
import { sha256 } from '@noble/hashes/sha256'
import { uploadAnywhereFile } from '../src/anywhere-upload'

test('resumes after a lost response and verifies chunks and whole file', async () => {
  const files = new Map<string, { bytes: number[]; total: number; hash: string }>()
  let lost = false
  const fetchImpl: typeof fetch = async (input, init) => {
    const path = String(input)
    if (path === '/uploads') {
      const body = JSON.parse(String(init?.body))
      const uploadId = String(files.size + 1)
      files.set(uploadId, { bytes: [], total: body.byteLength, hash: body.sha256 })
      return Response.json({ uploadId, received: 0, total: body.byteLength, sha256: body.sha256 })
    }
    const match = path.match(/^\/uploads\/(\d+)(.*)$/)!
    const uploadId = match[1], suffix = match[2], entry = files.get(uploadId)!
    if (suffix.startsWith('/chunks')) {
      const bytes = init!.body as Uint8Array
      assert.equal((init!.headers as Record<string, string>)['X-Momso-Chunk-Sha256'], Buffer.from(sha256(bytes)).toString('hex'))
      assert.equal(Number(new URL('https://fixture.invalid' + suffix).searchParams.get('offset')), entry.bytes.length)
      entry.bytes.push(...bytes)
      if (!lost) { lost = true; throw new TypeError('connection lost') }
    } else if (suffix === '/finish') {
      assert.equal(Buffer.from(sha256(new Uint8Array(entry.bytes))).toString('hex'), entry.hash)
      return Response.json({ complete: true })
    }
    return Response.json({ uploadId, received: entry.bytes.length, total: entry.total, sha256: entry.hash })
  }
  const [a, b] = await Promise.all([
    uploadAnywhereFile(new File(['one'], 'one.txt'), { fetchImpl }),
    uploadAnywhereFile(new File(['two'], 'two.txt'), { fetchImpl }),
  ])
  assert.deepEqual(a, { complete: true })
  assert.deepEqual(b, { complete: true })
  assert.equal(files.size, 2)
  await assert.rejects(uploadAnywhereFile(new File(['bad'], 'wrong.txt'), { fetchImpl, uploadId: '1' }), /upload_file_mismatch/)
})

test('waits while another large write holds the disk reservation, but not when space is low', async () => {
  let busyChunks = 2, busyFinish = 1, received = 0
  const sleeps: number[] = []
  const fetchImpl: typeof fetch = async (input, init) => {
    const path = String(input)
    if (path === '/uploads') return Response.json({ uploadId: '1', received: 0, total: 3, sha256: JSON.parse(String(init?.body)).sha256 })
    if (path.includes('/chunks')) {
      if (busyChunks-- > 0) return Response.json({ error: 'reservation_busy' }, { status: 409 })
      received = 3
      return Response.json({ uploadId: '1', received, total: 3 })
    }
    if (path.endsWith('/finish')) {
      if (busyFinish-- > 0) return Response.json({ error: 'reservation_busy' }, { status: 409 })
      return Response.json({ complete: true })
    }
    throw new Error('unexpected ' + path)
  }
  const sleep = async (ms: number) => { sleeps.push(ms) }
  assert.deepEqual(await uploadAnywhereFile(new File(['abc'], 'a.txt'), { fetchImpl, sleep }), { complete: true })
  assert.deepEqual(sleeps, [2000, 4000, 2000])

  const low: typeof fetch = async (input, init) => String(input) === '/uploads'
    ? Response.json({ uploadId: '2', received: 0, total: 3, sha256: JSON.parse(String(init?.body)).sha256 })
    : Response.json({ error: 'disk_low' }, { status: 507 })
  sleeps.length = 0
  await assert.rejects(uploadAnywhereFile(new File(['abc'], 'b.txt'), { fetchImpl: low, sleep }), /disk_low/)
  assert.deepEqual(sleeps, [])
})
