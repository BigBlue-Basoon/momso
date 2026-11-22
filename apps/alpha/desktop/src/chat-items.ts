// 한 채팅방에서 다룬 문서와 미디어(오른쪽 패널). 계약: archive-backend-port-notes.md "2026-11-16 옮긴 것 › 채팅방에서 다룬 문서와 미디어"
//   chat_items({ chatId }) → { documents: [{ id, title, summary, kind, status, updatedAt, how, by, at }],
//                              media: [{ id, kind, title, createdAt, how, by, at, path }],
//                              files: [{ id: 'sha256:…', title, createdAt, how: 'attached', by, at, path }] }  ← T42: 미디어가 아닌 첨부(글 파일 등)
//   how: 문서 'created' | 'edited' | 'read', 미디어 'created' | 'edited' | 'attached'. 새 기록부터 채워진다(AI 답 전후 파일 비교). 옛 기록은 null.
//   by: { kind: 'ai', provider, model(없으면 null) } | { kind: 'user' } | null. 겹쳐 답한 채팅방이 있어 누가 했는지 가를 수 없으면 null. at 도 null 일 수 있다.
//   evidence: 'tool'(도구가 성공을 알림) | 'diff'(파일 전후 비교로 확인) | null(옛 기록). 값만 받고 화면에는 보이지 않는다(보일지는 그가 정하지 않음).
//   null 인 칸은 화면이 비워 둔다(짐작해 채우지 않는다. by 가 null 이면 누가 했다고 적지 않는다).
//   이벤트 chat-items-updated { chatId }: 첨부, 채팅방 번호가 든 작업 내역, AI 답 완료 때.
import { invoke, listen } from './host'
import type { AiProvider } from './ai-models'
import type { MediaItem } from './media'
import { applySourceAssetImport, isDesktopRuntime, planSourceAssetImport, type SourceAssetImportProposal } from './runtime'
import { SOURCE_IMPORT_ACTOR, assetKindForFileName, fileNameFromPath, mediaTypeForFileName } from './source-import'

export type ItemBy = { kind: 'ai'; provider: AiProvider; model?: string | null } | { kind: 'user' }

export type ChatDocumentItem = {
  id: string
  title: string
  summary: string
  kind: string
  status: string
  updatedAt: string
  how: 'created' | 'edited' | 'read' | null
  by: ItemBy | null
  at: string | null
  evidence?: 'tool' | 'diff' | null
}

/** 미디어가 아닌 첨부파일(글 파일 등). path 는 몸소 폴더 안 원자료 경로. */
export type ChatFileItem = {
  id: string
  title: string
  createdAt: string | null
  how: 'attached' | null
  by: ItemBy | null
  at: string | null
  path: string | null
}

export type ChatItems = { documents: ChatDocumentItem[]; media: MediaItem[]; files: ChatFileItem[] }

export const EMPTY_CHAT_ITEMS: ChatItems = { documents: [], media: [], files: [] }

export async function getChatItems(chatId: string) {
  if (!isDesktopRuntime()) throw new Error('chat_items needs the desktop runtime')
  const value = await invoke<Partial<ChatItems> | null>('chat_items', { chatId })
  return { documents: value?.documents ?? [], media: value?.media ?? [], files: value?.files ?? [] } as ChatItems
}

/**
 * 입력 칸의 클립(끌어 놓기도 같다): 고른 파일을 원자료로 보존하고(plan/apply_source_asset_import) 이 채팅방에 첨부한다(chat_attach_asset).
 * 2026-11-16 본부 결정(클립 = 이 채팅방에 파일 첨부, 낱말은 "첨부"). 채팅방이 있어야 첨부된다(없으면 "채팅방을 찾을 수 없습니다").
 * 첨부한 것은 오른쪽 패널의 "이 채팅방에서 다룬 것"에 보인다(chat-items-updated). 미디어는 미디어 칸, 그 밖(글 파일 등)은 첨부파일 칸.
 * T42 부터 뒤쪽은 첨부한 파일을 다음 chat_send 의 말과 함께 묶어, 코덱스·클로드에 몸소 폴더 안 원자료 경로를 넘겨 읽게 한다(제미나이는 아직 아님).
 */
/** 내 말풍선에 보일 첨부 하나(2026-11-18). assetId 는 원자료 번호. */
export type AttachedFile = { assetId: string; name: string; mediaType: string | null; deleted?: boolean }

/**
 * 첨부의 앞 절반: 고른 파일을 원자료로 보존만 한다(채팅방이 없어도 된다). 폴더면 뒤쪽이 source_folder_not_allowed 로 거절한다.
 * 새 채팅방(아직 방이 없음)에서는 이것만 하고, 방은 첫 말을 보낼 때 만든 뒤 linkAttachment 로 잇는다(2026-11-20 본부: 방은 첫 말이 저장될 때만).
 */
export async function importAttachment(path: string): Promise<AttachedFile> {
  if (!isDesktopRuntime()) throw new Error('attach needs the desktop runtime')
  const fileName = fileNameFromPath(path)
  const proposal: SourceAssetImportProposal = {
    sourcePath: path,
    assetKind: assetKindForFileName(fileName),
    mediaType: mediaTypeForFileName(fileName),
    actor: SOURCE_IMPORT_ACTOR,
  }
  const plan = await planSourceAssetImport(proposal)
  const asset = await applySourceAssetImport({ ...proposal, assetKind: plan.assetKind }, plan.planDigest, true)
  return { assetId: asset.id, name: fileName, mediaType: proposal.mediaType ?? null }
}

/** 첨부의 뒤 절반: 보존한 원자료를 있는 채팅방에 잇는다(chat_attach_asset). */
export async function linkAttachment(chatId: string, assetId: string) {
  if (!isDesktopRuntime()) throw new Error('attach needs the desktop runtime')
  await invoke<string>('chat_attach_asset', { chatId, assetId })
}

export async function attachFileToChat(chatId: string, path: string): Promise<AttachedFile> {
  const file = await importAttachment(path)
  await linkAttachment(chatId, file.assetId)
  return file
}

/**
 * chat_read 의 내 말에 붙은 첨부(뒤쪽에 요청한 꼴, 2026-11-18): messages[].attachments = [{ assetId, name, mediaType, deleted }].
 * 뒤쪽은 보낼 때 첨부를 그 말에 묶어 둔다(chat_attachments.message_id). 아직 chat_read 가 돌려주지 않으면 null.
 */
export function readMessageAttachments(value: unknown): AttachedFile[] | null {
  if (!Array.isArray(value)) return null
  return value.flatMap((item) => {
    const entry = item as Partial<AttachedFile> | null
    if (!entry || typeof entry.assetId !== 'string' || typeof entry.name !== 'string') return []
    return [{ assetId: entry.assetId, name: entry.name, mediaType: typeof entry.mediaType === 'string' ? entry.mediaType : null, deleted: entry.deleted === true }]
  })
}

export async function listenChatItems(onChange: (chatId: string) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ chatId?: string | null }>('chat-items-updated', (event) => {
    if (event.payload?.chatId) onChange(event.payload.chatId)
  })
}

/**
 * 첨부파일의 글(T46·T48): attachment_text_read({ assetId, cursor }) → { state, content, nextCursor, totalBytes }.
 * 첫 쪽은 cursor 를 빼거나 0, 다음 쪽은 받은 nextCursor(없으면 끝). 한 번에 최대 64KiB, 한글이 잘리지 않게 뒤쪽이 위치를 맞춘다.
 * state: ok | not_found | not_text | invalid_cursor | invalid_encoding. UTF-8 글만 읽는다.
 */
export type AttachmentTextPage = {
  state: 'ok' | 'not_found' | 'not_text' | 'invalid_cursor' | 'invalid_encoding' | string
  content?: string | null
  nextCursor?: number | null
  totalBytes?: number | null
}

export async function readAttachmentText(assetId: string, cursor?: number | null) {
  if (!isDesktopRuntime()) throw new Error('attachment_text_read needs the desktop runtime')
  return invoke<AttachmentTextPage>('attachment_text_read', cursor ? { assetId, cursor } : { assetId })
}

export function chatItemCount(items: ChatItems | null) {
  return items ? items.documents.length + items.media.length + items.files.length : 0
}
