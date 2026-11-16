import { W } from './words'
import type {
  CreateRecordInput,
  LocalRecordDetail,
  LocalRecordStatus,
  RecordChangeProposal,
} from './runtime'
import type {
  RecordStatus,
  SummarySection,
  TranscriptLine,
  WorkspaceRecord,
} from './workbench-data'

const RECORD_FORMAT = 'momso-workspace-record-v1'
const RECORD_ACTOR = 'momso-desktop'

type PersistedWorkspaceFields = {
  preview: string
  dateLabel: string
  time: string
  duration: string
  sourceType: WorkspaceRecord['sourceType']
  sourceLabel: string
  participantLabel: string
  summarySections: SummarySection[]
  transcript: TranscriptLine[]
  createdAt: string
  checksum: string
  aiNote: string
  originalFileName?: string
  originalFileSize?: string
  retentionLabel?: string
  externalProvider?: string
  externalConversationId?: string
  projectLabel?: string
}

type PersistedWorkspaceRecord = {
  format: typeof RECORD_FORMAT
  fields: PersistedWorkspaceFields
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function optionalString(value: unknown) {
  return value === undefined || typeof value === 'string'
}

function isSummarySection(value: unknown): value is SummarySection {
  return isObject(value)
    && typeof value.title === 'string'
    && typeof value.body === 'string'
    && typeof value.source === 'string'
}

function isTranscriptLine(value: unknown): value is TranscriptLine {
  return isObject(value)
    && typeof value.time === 'string'
    && typeof value.speaker === 'string'
    && typeof value.text === 'string'
    && (value.important === undefined || typeof value.important === 'boolean')
}

function parsePersistedFields(body: string): PersistedWorkspaceFields | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(body)
  } catch {
    return null
  }
  if (!isObject(parsed) || parsed.format !== RECORD_FORMAT || !isObject(parsed.fields)) return null
  const fields = parsed.fields
  if (
    typeof fields.preview !== 'string'
    || typeof fields.dateLabel !== 'string'
    || typeof fields.time !== 'string'
    || typeof fields.duration !== 'string'
    || !['audio', 'note', 'ai-conversation'].includes(String(fields.sourceType))
    || typeof fields.sourceLabel !== 'string'
    || typeof fields.participantLabel !== 'string'
    || !Array.isArray(fields.summarySections)
    || !fields.summarySections.every(isSummarySection)
    || !Array.isArray(fields.transcript)
    || !fields.transcript.every(isTranscriptLine)
    || typeof fields.createdAt !== 'string'
    || typeof fields.checksum !== 'string'
    || typeof fields.aiNote !== 'string'
    || !optionalString(fields.originalFileName)
    || !optionalString(fields.originalFileSize)
    || !optionalString(fields.retentionLabel)
    || !optionalString(fields.externalProvider)
    || !optionalString(fields.externalConversationId)
    || !optionalString(fields.projectLabel)
  ) {
    return null
  }
  return fields as PersistedWorkspaceFields
}

function persistedFields(record: WorkspaceRecord): PersistedWorkspaceFields {
  return {
    preview: record.preview,
    dateLabel: record.dateLabel,
    time: record.time,
    duration: record.duration,
    sourceType: record.sourceType,
    sourceLabel: record.sourceLabel,
    participantLabel: record.participantLabel,
    summarySections: record.summarySections,
    transcript: record.transcript,
    createdAt: record.createdAt,
    checksum: record.checksum,
    aiNote: record.aiNote,
    ...(record.originalFileName ? { originalFileName: record.originalFileName } : {}),
    ...(record.originalFileSize ? { originalFileSize: record.originalFileSize } : {}),
    ...(record.retentionLabel ? { retentionLabel: record.retentionLabel } : {}),
    ...(record.externalProvider ? { externalProvider: record.externalProvider } : {}),
    ...(record.externalConversationId
      ? { externalConversationId: record.externalConversationId }
      : {}),
    ...(record.projectLabel ? { projectLabel: record.projectLabel } : {}),
  }
}

export function serializeWorkspaceRecord(record: WorkspaceRecord) {
  const payload: PersistedWorkspaceRecord = {
    format: RECORD_FORMAT,
    fields: persistedFields(record),
  }
  return JSON.stringify(payload)
}

function truncateCharacters(value: string, maximum: number) {
  const characters = Array.from(value)
  return characters.length <= maximum ? value : characters.slice(0, maximum).join('')
}

export function summarizeWorkspaceRecord(record: WorkspaceRecord) {
  const combined = record.summarySections
    .map((section) => `${section.title}\n${section.body}`)
    .join('\n\n')
    .trim()
  const summary = combined || record.preview.trim()
  return summary ? truncateCharacters(summary, 4_000) : null
}

function recordKind(record: WorkspaceRecord) {
  if (record.runtimeRecordKind) return record.runtimeRecordKind
  if (record.sourceType === 'audio') return 'audio_record'
  if (record.sourceType === 'ai-conversation') return 'ai_conversation'
  return 'note'
}

function toRuntimeStatus(status: RecordStatus): LocalRecordStatus {
  if (status === 'needs-review') return 'needs_review'
  return status
}

function toWorkspaceStatus(status: LocalRecordStatus): RecordStatus {
  if (status === 'needs_review') return 'needs-review'
  return status
}

export function workspaceRecordCreateInput(record: WorkspaceRecord): CreateRecordInput {
  return {
    recordKind: recordKind(record),
    title: record.title,
    body: serializeWorkspaceRecord(record),
    summary: summarizeWorkspaceRecord(record),
    actor: RECORD_ACTOR,
    initialStatus: record.status === 'draft' ? 'draft' : 'needs_review',
  }
}

export function workspaceRecordChangeProposal(
  record: WorkspaceRecord,
  targetStatus: LocalRecordStatus = toRuntimeStatus(record.status),
): RecordChangeProposal {
  if (!record.runtimeVersion) throw new Error(W.recordPersistence.text1)
  return {
    recordId: record.id,
    expectedVersion: record.runtimeVersion,
    recordKind: recordKind(record),
    title: record.title,
    body: serializeWorkspaceRecord(record),
    summary: summarizeWorkspaceRecord(record),
    targetStatus,
    actor: RECORD_ACTOR,
  }
}

function shortChecksum(hash: string) {
  return `SHA-256 · ${hash.slice(0, 8)}…${hash.slice(-8)}`
}

function fallbackFields(detail: LocalRecordDetail): PersistedWorkspaceFields {
  return {
    preview: (detail.summary ?? detail.body).replace(/\s+/g, ' ').slice(0, 90),
    dateLabel: detail.updatedAt.slice(0, 10),
    time: detail.updatedAt.slice(11, 16),
    duration: `${detail.bodyBytes.toLocaleString('ko-KR')} bytes`,
    sourceType: 'note',
    sourceLabel: W.recordPersistence.text2,
    participantLabel: detail.updatedBy,
    summarySections: detail.summary
      ? [{ title: W.recordPersistence.text3, body: detail.summary, source: W.recordPersistence.text4 }]
      : [],
    transcript: detail.body
      ? [{ time: W.recordPersistence.text5, speaker: W.recordPersistence.text6, text: detail.body }]
      : [],
    createdAt: detail.createdAt,
    checksum: shortChecksum(detail.bodySha256),
    aiNote: W.recordPersistence.text7,
  }
}

export function workspaceRecordFromDetail(detail: LocalRecordDetail): WorkspaceRecord {
  const fields = parsePersistedFields(detail.body) ?? fallbackFields(detail)
  return {
    id: detail.id,
    title: detail.title,
    ...fields,
    status: toWorkspaceStatus(detail.status),
    archiveId: `momso-local / ${detail.id}`,
    runtimeRecordKind: detail.recordKind,
    runtimeVersion: detail.currentVersion,
    runtimeBodySha256: detail.bodySha256,
  }
}
