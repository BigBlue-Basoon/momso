// 원자료 열기와 지우기. 계약: archive-backend-port-notes.md "T59: 원자료 열기와 삭제", docs/development/desktop-object-store-recovery-2026-11-16.md "원자료 삭제와 앱 상태 복원 연결".
//   원자료 번호는 내부 번호, sha256: 참조, 해시를 모두 받는다(미디어 목록의 id 는 sha256: 참조).
//   source_asset_open({ assetId, cursor }) → { state, kind: 'text'|'image'|'video'|'audio'|null, path, content, nextCursor, totalBytes }
//     글이면 첨부파일과 같은 읽기(attachment_text_read 와 같은 구현). 사진·소리·영상이면 미디어 화면과 같은 path. 지워진 자료는 열지 않는다.
//   source_asset_delete_preview({ assetId }) → { assetId, originalFileName, state, documents, receipts, chats, recordVersions, localRemaining, remoteRemaining, planDigest }
//   source_asset_trash({ assetId, planDigest }) → { state, assetId, retainUntil }   되살릴 수 있게 지운다(최소 30일 보관, 자동 삭제 없음).
//   source_asset_restore_trashed({ assetId }) → { state, assetId, retainUntil }
//   source_asset_purge({ assetId, planDigest, confirmed: true }) → { state: 'partial'…, localRemaining, remoteRemaining, removedRemoteKeys }
//     지금은 확인할 수 없는 사본 때문에 늘 partial 이다(원격·이전 버전·외부 사본은 완전 삭제를 보장하지 않는다).
//   오류: references_changed(쓰는 곳이 바뀜, 다시 미리 본다), chat_active(답하는 중), 그 밖은 지우지 못함.
import { invoke } from './host'
import { hasAppBackend, isDesktopRuntime } from './runtime'

export type SourceOpen = {
  state: string
  kind: 'text' | 'image' | 'video' | 'audio' | null
  path: string | null
  content: string | null
  nextCursor: number | null
  totalBytes: number | null
}

export type SourceDeletePreview = {
  assetId: string
  originalFileName: string | null
  state: string
  documents: string[]
  receipts: string[]
  chats: string[]
  recordVersions: Array<{ recordId: string; recordVersion: number }>
  planDigest: string
  /** 사용처를 알아본 결과인가. 휴대폰 몸소의 server 는 사용처를 알려 주지 않아 false(묻는 창이 "쓰는 곳이 없습니다"라고 하지 않는다). */
  usesKnown: boolean
}

export type SourcePurgeResult = {
  state: string
  localRemaining: string[]
  remoteRemaining: string[]
  removedRemoteKeys: string[]
  /** 휴대폰 몸소: 이미 올라간 예전 백업에는 사본이 남을 수 있다(S83). 데스크톱은 false. */
  backupCopiesRemain: boolean
}

function need() {
  if (!isDesktopRuntime()) throw new Error('source assets need the desktop runtime')
}
/** 삭제(휴지통)·되돌리기·삭제한 원자료 목록·완전히 삭제는 휴대폰 몸소에서도(S75 셋째·S83, server /assets/trash·purge). */
function needAny() {
  if (!hasAppBackend()) throw new Error('source assets need the desktop runtime')
}

export async function openSourceAsset(assetId: string) {
  need()
  return invoke<SourceOpen>('source_asset_open', { assetId })
}

export async function previewSourceDelete(assetId: string) {
  needAny()
  const value = await invoke<(Partial<SourceDeletePreview> & { usesUnknown?: boolean }) | null>('source_asset_delete_preview', { assetId })
  if (!value || typeof value.planDigest !== 'string') throw new Error('unavailable')
  return {
    assetId: value.assetId ?? assetId,
    originalFileName: value.originalFileName ?? null,
    state: value.state ?? 'active',
    documents: Array.isArray(value.documents) ? value.documents : [],
    receipts: Array.isArray(value.receipts) ? value.receipts : [],
    chats: Array.isArray(value.chats) ? value.chats : [],
    recordVersions: Array.isArray(value.recordVersions) ? value.recordVersions : [],
    planDigest: value.planDigest,
    usesKnown: value.usesUnknown !== true,
  } satisfies SourceDeletePreview
}

/**
 * 원자료가 삭제·되돌리기·완전히 삭제로 바뀌었다고 화면 전체에 알린다(2026-11-18).
 * 뒤쪽은 이때 chat-items-updated 를 보내지 않아, 설정에서 되돌린 원자료가 채팅방을 옮긴 뒤에야 첨부 목록에 보였다.
 * App 이 지금 채팅방의 첨부 목록과 말풍선의 첨부 표시를 다시 그린다. 어느 화면에서 바꿨든 이 세 함수를 거치므로 한곳에서 알린다.
 */
export type SourceChange = { assetId: string; deleted: boolean }
function announce(change: SourceChange) {
  window.dispatchEvent(new CustomEvent<SourceChange>('momso:source-changed', { detail: change }))
}

export async function trashSource(assetId: string, planDigest: string) {
  needAny()
  await invoke('source_asset_trash', { assetId, planDigest })
  announce({ assetId, deleted: true })
}

export async function restoreTrashedSource(assetId: string) {
  needAny()
  await invoke('source_asset_restore_trashed', { assetId })
  announce({ assetId, deleted: false })
}

export async function purgeSource(assetId: string, planDigest: string) {
  needAny()
  const value = await invoke<Partial<SourcePurgeResult> | null>('source_asset_purge', { assetId, planDigest, confirmed: true })
  announce({ assetId, deleted: true })
  return {
    state: value?.state ?? 'partial',
    localRemaining: Array.isArray(value?.localRemaining) ? value.localRemaining : [],
    remoteRemaining: Array.isArray(value?.remoteRemaining) ? value.remoteRemaining : [],
    removedRemoteKeys: Array.isArray(value?.removedRemoteKeys) ? value.removedRemoteKeys : [],
    backupCopiesRemain: value?.backupCopiesRemain === true,
  } satisfies SourcePurgeResult
}

/** 뒤쪽 오류 글에서 정해 둔 값만 꺼낸다. */
export function sourceErrorCode(error: unknown) {
  const text = error instanceof Error ? error.message : String(error)
  return (['references_changed', 'chat_active', 'invalid_state', 'source_deleted'] as const).find((code) => text.includes(code)) ?? null
}

/**
 * 삭제한 원자료 목록(2026-11-17 뒤쪽 2ef0fd2f). source_asset_deleted_list({ before, limit }) → { items, hasMore, nextCursor }.
 *   items: 원자료(id, originalFileName …) + deletedAt, deletionState, canRestore, canPurge, referenceCount, referenceCounts, localRemaining, remoteRemaining.
 *   이어 받을 때는 앞의 nextCursor({ deletedAt, assetId })를 before 로 준다. 한 번에 40개.
 */
export type DeletedSource = {
  id: string
  originalFileName: string | null
  deletedAt: string | null
  deletionState: string
  canRestore: boolean
  canPurge: boolean
  /** 쓰던 곳 수. 휴대폰 몸소에서 server 가 문서 수를 읽지 못하면 null(줄에 수를 적지 않는다). */
  referenceCount: number | null
  /** 휴대폰 몸소: 완전히 삭제해도 예전 백업에는 사본이 남을 수 있다(S83). 데스크톱은 false. */
  backupCopiesRemain: boolean
}
export type DeletedSourceCursor = { deletedAt: string; assetId: string }
export type DeletedSourcePage = { items: DeletedSource[]; hasMore: boolean; nextCursor: DeletedSourceCursor | null }

export async function listDeletedSources(before: DeletedSourceCursor | null = null): Promise<DeletedSourcePage> {
  needAny()
  const value = await invoke<{ items?: Array<Partial<DeletedSource>>; hasMore?: boolean; nextCursor?: DeletedSourceCursor | null } | null>(
    'source_asset_deleted_list', before ? { before } : {},
  )
  const items = (Array.isArray(value?.items) ? value.items : []).filter((item) => typeof item.id === 'string').map((item) => ({
    id: item.id as string,
    originalFileName: item.originalFileName ?? null,
    deletedAt: item.deletedAt ?? null,
    deletionState: item.deletionState ?? '',
    canRestore: item.canRestore === true,
    canPurge: item.canPurge === true,
    referenceCount: typeof item.referenceCount === 'number' ? item.referenceCount : item.referenceCount === null ? null : 0,
    backupCopiesRemain: item.backupCopiesRemain === true,
  }))
  const nextCursor = value?.nextCursor && typeof value.nextCursor.assetId === 'string' ? value.nextCursor : null
  return { items, hasMore: value?.hasMore === true && nextCursor !== null, nextCursor }
}
