import { W } from './words'
// 보관함(읽는 글, 원본, 작업 기록) 명령을 부르는 얇은 층.
// 화면은 이 파일의 함수만 쓴다. 명령의 모양은 momso-public-prep/archive-backend-port-notes.md 를 따른다.
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { isDesktopRuntime } from './runtime'

export type ZetKind = 'summary' | 'transcript' | 'note'
export type ZetStatus = 'draft' | 'confirmed'
export type ZetEdgeType = 'summarizes' | 'derived_from' | 'continues' | 'references' | 'supersedes'

export type ZetListEntry = {
  id: string
  title: string
  summary: string
  kind: ZetKind
  status: ZetStatus
  createdAt: string
  updatedAt: string
}

// 글 머리의 시각 키는 목록과 달리 created_at, updated_at 이다.
export type ZetHead = {
  id: string
  title: string
  summary: string
  kind: ZetKind
  status: ZetStatus
  created_at: string
  updated_at: string
  assets: string[]
  edges: Array<{ type: ZetEdgeType; target: string }>
}

export type ZetDocument = {
  head: ZetHead
  body: string
  /** 읽은 그때의 글 내용을 가리키는 지문. 저장할 때 돌려주면 그사이 글이 바뀌었는지 백엔드가 알아본다. */
  contentHash: string
}

/** 이어진 글 하나. 글이 없어졌으면 missing 이 true 이고 제목·종류·상태는 null 이다. */
export type ZetLinkedEntry = {
  type: ZetEdgeType
  id: string
  title: string | null
  zetKind: ZetKind | null
  status: ZetStatus | null
  missing: boolean
}

export type ZetLinkedAsset = {
  id: string
  originalFileName: string | null
}

export type ZetLinkedReceipt = {
  id: string
  createdAt: string
  tool: ReceiptTool
  model: string
}

export type ZetLinked = {
  assets: ZetLinkedAsset[]
  outgoing: ZetLinkedEntry[]
  incoming: ZetLinkedEntry[]
  receipts: ZetLinkedReceipt[]
}

/** 저장한 판 하나. title 은 그 판에서의 이 글의 제목, otherZets 는 같이 저장된 다른 글의 수. */
export type ZetHistoryEntry = {
  revision: string
  time: string
  title: string | null
  otherZets: number
}

/** 지난 판의 내용. 머리를 읽을 수 없는 판은 head 가 null 이고 body 에 파일 글자 전체가 온다. */
export type ZetVersion = {
  head: ZetHead | null
  body: string
  headUnreadable: boolean
}

export type ReceiptTool = 'codex' | 'claude' | 'gemini' | 'person'

/** 작업 기록이 읽거나 쓴 것 하나. 글이면 제목·종류·상태가, 원본이면 원래 파일 이름이 같이 온다. */
export type ReceiptReference =
  | {
      type: 'zet'
      id: string
      title: string | null
      zetKind: ZetKind | null
      status: ZetStatus | null
      missing: boolean
    }
  | {
      type: 'asset'
      id: string
      originalFileName: string | null
    }

export type ReceiptHead = {
  id: string
  created_at: string
  tool: ReceiptTool
  model: string
  read: ReceiptReference[]
  wrote: ReceiptReference[]
}

export type ReceiptDocument = {
  head: ReceiptHead
  body: string
}

export type ArchiveStatus = {
  path: string
  zetCount: number
  receiptCount: number
  hasUnsavedChanges: boolean
  usesBundledGit: boolean
}

const ZET_ID = /zet_\d{8}_\d{6}/

export function isArchiveAvailable() {
  return isDesktopRuntime()
}

/** 글자 안에서 글 번호를 찾는다. 없으면 null. */
export function findZetId(text: string) {
  return ZET_ID.exec(text)?.[0] ?? null
}

/**
 * 명령이 실패하면 백엔드가 한글 문장을 돌려준다. 그 문장은 그대로 보여 준다.
 * 한글이 없는 문장(명령이 아직 없을 때 등)은 화면에 내보이지 않고 fallback 을 쓴다.
 */
export function archiveErrorMessage(error: unknown, fallback: string) {
  const message = typeof error === 'string'
    ? error
    : error instanceof Error
      ? error.message
      : ''
  return /[가-힣]/.test(message) ? message : fallback
}

function requireArchive() {
  if (!isArchiveAvailable()) throw new Error('archive commands need the desktop runtime')
}

export async function listZets() {
  requireArchive()
  return invoke<ZetListEntry[]>('zet_list', { kind: null, status: null })
}

export async function readZet(id: string) {
  requireArchive()
  return invoke<ZetDocument>('zet_read', { id })
}

/**
 * 글을 고쳐서 저장한다: 파일에 쓰고 그 글 하나를 이력에 남긴다(지난 판이 하나 생긴다). 한 번에 되거나 안 된다.
 * expectedHash 는 글을 읽을 때 받은 contentHash 다. 그사이 글이 바뀌었으면 쓰지 않고 실패한다(isChangedMeanwhileError).
 */
export async function saveZet(
  id: string,
  changes: { title?: string; summary?: string; body?: string },
  expectedHash: string,
) {
  requireArchive()
  await invoke<null>('zet_save', { id, changes, expectedHash })
}

export function isChangedMeanwhileError(error: unknown) {
  return error === '이 글이 그사이 바뀌었습니다'
}

export async function readZetLinked(id: string) {
  requireArchive()
  return invoke<ZetLinked>('zet_linked', { id })
}

export async function readZetHistory(id: string) {
  requireArchive()
  return invoke<ZetHistoryEntry[]>('zet_history', { id })
}

export async function readZetVersion(id: string, revision: string) {
  requireArchive()
  return invoke<ZetVersion>('zet_read_version', { id, revision })
}

/**
 * 지난 판으로 되돌린다. 그 글에 저장하지 않은 내용이 있으면 되돌리지 않고 실패한다(isUnsavedError).
 * 사람이 그 내용을 버리기로 한 뒤에만 discardUnsaved 를 true 로 준다.
 */
export async function restoreZetVersion(id: string, revision: string, discardUnsaved = false) {
  requireArchive()
  await invoke<null>('zet_restore_version', { id, revision, discardUnsaved })
}

export function isUnsavedError(error: unknown) {
  return error === '저장하지 않은 내용이 있습니다'
}

export function isWebAddress(address: string) {
  return /^https?:\/\//i.test(address)
}

/** 바깥 주소(http, https)를 기본 브라우저에서 연다. 앱 화면은 다른 곳으로 넘어가지 않는다. */
/**
 * 원자료 찾기: source_search({ query }) → [{ id, originalFileName, mediaType, size, createdAt, linkedDocumentIds }].
 * 원자료의 파일 이름에 찾는 말이 든 것(대소문자 무시), 새것부터. linkedDocumentIds 는 그 원자료와 이어진 문서 번호들.
 */
export type SourceSearchResult = {
  id: string
  originalFileName: string
  mediaType: string | null
  size: number
  createdAt: string
  linkedDocumentIds: string[]
}

export async function searchSources(query: string) {
  if (!isDesktopRuntime()) throw new Error('source_search needs the desktop runtime')
  return invoke<SourceSearchResult[]>('source_search', { query })
}

export async function openWebAddress(address: string) {
  if (!isWebAddress(address)) throw new Error(W.archive.text1)
  await openUrl(address)
}

export async function readReceipt(id: string) {
  requireArchive()
  return invoke<ReceiptDocument>('receipt_read', { id })
}

export async function getArchiveStatus() {
  requireArchive()
  return invoke<ArchiveStatus>('archive_status')
}
