import assert from 'node:assert/strict'
import test from 'node:test'
import {
  serializeWorkspaceRecord,
  workspaceRecordChangeProposal,
  workspaceRecordCreateInput,
  workspaceRecordFromDetail,
} from '../src/record-persistence'
import { WORKSPACE_RECORDS } from '../src/workbench-data'
import type { LocalRecordDetail } from '../src/runtime'

test('가져온 기록은 SQLite 본문에서 손실 없이 다시 구성된다', () => {
  const imported = {
    ...WORKSPACE_RECORDS[0],
    id: 'temporary-import-id',
    status: 'needs-review' as const,
  }
  const input = workspaceRecordCreateInput(imported)
  const detail: LocalRecordDetail = {
    id: '11111111-1111-4111-8111-111111111111',
    recordKind: input.recordKind,
    title: input.title,
    status: 'needs_review',
    currentVersion: 1,
    summary: input.summary,
    summaryState: 'unconfirmed',
    bodyBytes: new TextEncoder().encode(input.body).byteLength,
    updatedAt: '2026-07-17T01:02:03.000Z',
    body: input.body,
    bodySha256: 'a'.repeat(64),
    createdBy: 'momso-desktop',
    updatedBy: 'momso-desktop',
    reviewedBy: null,
    reviewedAt: null,
    createdAt: '2026-07-17T01:02:03.000Z',
  }

  const restored = workspaceRecordFromDetail(detail)

  assert.equal(restored.id, detail.id)
  assert.equal(restored.runtimeVersion, 1)
  assert.equal(restored.status, 'needs-review')
  assert.deepEqual(restored.summarySections, imported.summarySections)
  assert.deepEqual(restored.transcript, imported.transcript)
  assert.equal(serializeWorkspaceRecord(restored), input.body)
})

test('저장 변경안은 현재 SQLite 버전에 묶여 낡은 덮어쓰기를 막는다', () => {
  const record = {
    ...WORKSPACE_RECORDS[0],
    id: '11111111-1111-4111-8111-111111111111',
    status: 'reviewed' as const,
    runtimeRecordKind: 'audio_record',
    runtimeVersion: 7,
  }

  const proposal = workspaceRecordChangeProposal(record, 'reviewed')

  assert.equal(proposal.recordId, record.id)
  assert.equal(proposal.expectedVersion, 7)
  assert.equal(proposal.targetStatus, 'reviewed')
  assert.equal(proposal.actor, 'momso-desktop')
})
