// 대화(왼쪽 대화 목록, 가운데 대화 칸) 명령을 부르는 얇은 층.
// 모양은 본부가 전한 계약을 따른다(chat_list, chat_create, chat_read, chat_append, chat_rename, chat_search).
// 백엔드가 들어오면 momso-public-prep/archive-backend-port-notes.md 의 대화 절과 견준다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isMissingCommand, type AiProvider } from './ai-models'
import { isDesktopRuntime } from './runtime'

export type ChatRole = 'user' | 'assistant'

/** 목록의 대화 하나. 최근에 바뀐 것이 먼저 온다. */
export type ChatEntry = {
  id: string
  /** 아직 이름이 없으면 빈 글자다. 첫 사용자 말을 붙이면 백엔드가 앞 40자로 이름을 붙인다. */
  title: string
  createdAt: string
  updatedAt: string
  messageCount: number
  /** 들어 있는 프로젝트(없으면 null). 프로젝트 백엔드가 주는 칸. */
  groupId?: string | null
  /** 고정한 채팅방인지(chat_list). 고정한 것은 목록 맨 위에 고정한 차례(pinnedOrder)로 모인다. */
  pinned?: boolean | null
  pinnedOrder?: number | null
}

export type ChatMessage = {
  id: string
  role: ChatRole
  text: string
  createdAt: string
  /** AI 의 답이면 답한 곳과 모델(백엔드가 chat_send 로 붙인 답에만 있다). */
  provider?: AiProvider | null
  model?: string | null
  /** 그 답의 추론 수준(백엔드가 적은 것). */
  effort?: string | null
  /** 빠른 모드로 쓴 답인지(chat_read). 확인된 true/false, 모르면 null. true 일 때만 표시한다. */
  fast?: boolean | null
  /** 그 답의 실제 고속 상태(chat_read, 세 회사 계약 cef940fa): on / off / cooldown / null(모름, 옛 답). on 일 때만 표시한다. */
  fastState?: 'on' | 'off' | 'cooldown' | null
  /** 이 답을 쓸 때 다른 회사에 넘기지 못한 앞의 말 수(chat_read). 옛 말은 null. 1 이상일 때만 알린다. */
  notHandedOver?: number | null
  /** 사용자가 이 말에 고른 스킬의 부르는 이름(스킬 계약). */
  skill?: string | null
  /** 그 답에 실제로 쓰인 추론 수준(T43). 확인되지 않으면 null(제미나이는 지금 오지 않는다). */
  actualEffort?: string | null
  /** 답이 끝난 까닭(chat_read). 옛 말은 null. completed 가 아니면 끊긴 답이라 한 줄 알린다. */
  finishReason?: 'completed' | 'length_limit' | 'turn_limit' | 'cancelled' | 'error' | 'refusal' | null
  /** 내 말에 붙여 보낸 첨부(뒤쪽에 요청한 꼴. chat-items.ts readMessageAttachments 로 읽는다). */
  attachments?: unknown
}

export type ChatDocument = {
  id: string
  title: string
  createdAt: string
  updatedAt: string
  messages: ChatMessage[]
}

/** 찾기 결과. 말에서 찾았으면 그 둘레의 글이 snippet 으로 온다. */
export type ChatSearchResult = ChatEntry & {
  snippet?: string | null
}

function requireChats() {
  if (!isDesktopRuntime()) throw new Error('chat commands need the desktop runtime')
}

export async function listChats() {
  requireChats()
  return invoke<ChatEntry[]>('chat_list')
}

/**
 * 다른 AI 에서 내보낸 대화 가져오기: chat_import_export({ path, provider }) → 센 수.
 * provider: chatgpt | claude(제미나이는 unsupported_format). 파일은 그 회사가 준 zip 또는 conversations.json.
 * 실패 값: invalid_format, unsupported_format, unsupported_provider, archive_limit, file_error, storage_error.
 * 가져온 대화는 뒤쪽이 채팅방으로 만든다(importedFrom). 같은 대화는 duplicate, 새 말이 이어졌으면 updated.
 */
export type ChatImportSource = 'chatgpt' | 'claude' | 'gemini'
export type ChatImportCounts = {
  imported: number
  duplicate: number
  skipped: number
  omittedNonTextMessages: number
  omittedAttachments: number
  updated: number
  addedMessages: number
  conflicts: number
  /** 제미나이: 30분 규칙으로 채팅방을 나눴다는 표시(뒤쪽 글은 쓰지 않고 있다·없다만 본다). */
  grouped?: boolean
  /** 제미나이: HTML 을 골라 JSON 이 필요하다는 안내. */
  needsJson?: boolean
}

/**
 * 가져오기 전에 세기, 진행, 그만두기(desktop-screen-backend-contract.md "다른 AI의 대화 가져오기", chat-export-import-contract.md).
 *   chat_import_export_preview({ path, provider }) → { conversations, alreadyImported, unreadable, oldest, newest } (날짜는 UTC 글 또는 null, 저장하지 않음)
 *   chat_import_export_progress({ requestId, path, provider }) → chat_import_export 와 같은 센 수. 대화 하나마다 chat-import-progress { requestId, processed }.
 *   chat_import_export_cancel({ requestId }) → 대화 경계에서 멈추고 그 요청 전체를 되돌린다. 그러면 가져오기는 'cancelled' 오류로 끝난다.
 *   alreadyImported 는 같은 회사·대화 번호가 있는 수(새 말이 이어졌으면 더한다).
 */
export type ChatImportPreview = { conversations: number; alreadyImported: number; unreadable: number; oldest: string | null; newest: string | null; grouped?: boolean; needsJson?: boolean }
export async function previewChatExport(path: string, provider: ChatImportSource) {
  requireChats()
  const value = await invoke<(Partial<ChatImportPreview> & { groupingNotice?: unknown; guidance?: unknown }) | null>('chat_import_export_preview', { path, provider })
  const n = (v: unknown) => (typeof v === 'number' ? v : 0)
  return { conversations: n(value?.conversations), alreadyImported: n(value?.alreadyImported), unreadable: n(value?.unreadable), oldest: value?.oldest ?? null, newest: value?.newest ?? null, grouped: typeof value?.groupingNotice === 'string', needsJson: typeof value?.guidance === 'string' } as ChatImportPreview
}
export async function importChatExportWithProgress(requestId: string, path: string, provider: ChatImportSource, onProgress: (processed: number) => void) {
  requireChats()
  const stop = await listen<{ requestId: string; processed: number }>('chat-import-progress', (event) => {
    if (event.payload?.requestId === requestId) onProgress(event.payload.processed)
  })
  try {
    const value = await invoke<(Partial<ChatImportCounts> & { groupingNotice?: unknown; guidance?: unknown }) | null>('chat_import_export_progress', { requestId, path, provider })
    const n = (key: keyof ChatImportCounts) => (typeof value?.[key] === 'number' ? value[key] as number : 0)
    return {
      imported: n('imported'), duplicate: n('duplicate'), skipped: n('skipped'), omittedNonTextMessages: n('omittedNonTextMessages'),
      omittedAttachments: n('omittedAttachments'), updated: n('updated'), addedMessages: n('addedMessages'), conflicts: n('conflicts'),
      grouped: typeof value?.groupingNotice === 'string', needsJson: typeof value?.guidance === 'string',
    } as ChatImportCounts
  } finally {
    stop()
  }
}
export async function cancelChatImport(requestId: string) {
  requireChats()
  await invoke<null>('chat_import_export_cancel', { requestId })
}

/**
 * 채팅방 GitHub 백업(archive-backend-port-notes.md "채팅방 GitHub 백업 계약").
 *   chat_archive_redaction_count({ chatId }) → 마지막 사본에서 가린 부분의 수, 사본이 없으면 null(읽기 실패는 오류).
 *   chat-archive-changed { chatId, state: 'saved' | 'unavailable' } — 사본을 쓰고 나면 온다.
 */
export async function chatRedactionCount(chatId: string) {
  requireChats()
  const value = await invoke<number | null>('chat_archive_redaction_count', { chatId })
  return typeof value === 'number' ? value : null
}
export async function listenChatArchiveChanged(onChanged: (chatId: string | null, state: string | null) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ chatId?: string | null; state?: string | null }>('chat-archive-changed', (event) => onChanged(event.payload?.chatId ?? null, event.payload?.state ?? null))
}

/**
 * 채팅방 목록 쪽 나눔(2026-11-17 뒤쪽 fdce0939). chat_list_page({ offset }) → { items, hasMore, nextOffset }, 한 쪽 80개.
 * 차례는 chat_list 와 같다(고정한 것 먼저, 최근에 고친 순). 보관한 채팅방은 빠진다.
 * 명령이 없는 뒤쪽이면 옛 chat_list 전체를 한 쪽으로 돌려준다(한 번 없다고 알면 다시 묻지 않는다).
 */
export type ChatListPage = { items: ChatEntry[]; hasMore: boolean; nextOffset: number | null }
let listPageMissing = false
export async function listChatsPage(offset = 0): Promise<ChatListPage> {
  requireChats()
  if (!listPageMissing) {
    try {
      const value = await invoke<Partial<ChatListPage> | null>('chat_list_page', offset ? { offset } : {})
      const items = Array.isArray(value?.items) ? value.items : []
      const nextOffset = typeof value?.nextOffset === 'number' ? value.nextOffset : null
      return { items, hasMore: value?.hasMore === true && nextOffset !== null, nextOffset }
    } catch (error) {
      if (!isMissingCommand(error)) throw error
      listPageMissing = true
    }
  }
  if (offset) return { items: [], hasMore: false, nextOffset: null }
  return { items: await listChats(), hasMore: false, nextOffset: null }
}

export async function createChat() {
  requireChats()
  return invoke<ChatEntry>('chat_create')
}

/**
 * 채팅방과 내 첫 말을 한 번에 저장한다(2026-11-20 뒤쪽 f2b1547, desktop-screen-backend-contract.md "첫 말과 채팅방 만들기").
 * 빈 글은 empty_message, 첫 받아 오기 중이면 first_receive_in_progress, 저장 실패는 storage_error. 실패하면 방도 말도 남지 않는다.
 * 보내기는 그 뒤 chat_send 에 retryMessageId = messageId 로 한다(같은 말을 새로 저장하지 않고 첨부를 그 말에 묶는다).
 */
export async function createChatWithMessage(text: string, skill: string | null) {
  requireChats()
  return invoke<{ chat: ChatEntry; messageId: string }>('chat_create_with_message', { text, skill })
}

export async function readChat(id: string) {
  requireChats()
  return invoke<ChatDocument>('chat_read', { id })
}

/**
 * 긴 채팅방 페이지 읽기(2026-11-18 둘째 줄 B84, desktop-chat-page-b84.md): chat_read_page({ id, before?, limit? }) →
 * chat_read 와 같은 채팅방 + messages(그 범위 안에서 오래된 순) + nextCursor(그 앞으로 더 읽을 때 before 로, 더 없으면 null).
 * 기본 limit 100(1~200). 명령이 없는 예전 뒤쪽이면 chat_read 로 전체를 읽고 nextCursor 는 null.
 */
export async function readChatPage(id: string, before: string | null = null, limit = 100): Promise<{ document: ChatDocument; nextCursor: string | null }> {
  requireChats()
  try {
    const page = await invoke<ChatDocument & { nextCursor?: string | null }>('chat_read_page', { id, before, limit })
    const { nextCursor, ...document } = page
    return { document: document as ChatDocument, nextCursor: typeof nextCursor === 'string' && nextCursor ? nextCursor : null }
  } catch (error) {
    if (!isMissingCommand(error) || before) throw error
    return { document: await readChat(id), nextCursor: null }
  }
}

export async function renameChat(id: string, title: string) {
  requireChats()
  await invoke<null>('chat_rename', { id, title })
}

export async function searchChats(query: string) {
  requireChats()
  return invoke<ChatSearchResult[]>('chat_search', { query })
}

/**
 * 채팅 찾기 쪽 나눔(2026-11-17, 여섯째 줄 CHAT-SEARCH-PAGING-G33, 본부가 옮겨 준 알맹이).
 *   chat_search_page({ query, offset }) → { items, hasMore, nextOffset }. items 는 chat_search 와 같은 꼴, 한 번에 많아야 80개.
 *   처음엔 offset 0, 이어 받을 땐 앞의 nextOffset. 차례: 고정한 것 먼저, 최근에 고친 순, 같은 때면 ID 내림차순.
 *   결과가 바뀌는 동안 이어 받으면 겹치거나 빠질 수 있다 → 찾는 말이나 채팅방 목록이 바뀌면 처음부터 다시 찾는다.
 * 이 명령이 아직 없는 뒤쪽이면 옛 chat_search 의 배열을 한 쪽으로 돌려준다(hasMore false). 한 번 없다고 알면 다시 묻지 않는다.
 */
export type ChatSearchPage = { items: ChatSearchResult[]; hasMore: boolean; nextOffset: number | null }
let searchPageMissing = false
export async function searchChatsPage(query: string, offset = 0): Promise<ChatSearchPage> {
  requireChats()
  if (!searchPageMissing) {
    try {
      const value = await invoke<Partial<ChatSearchPage> | null>('chat_search_page', offset ? { query, offset } : { query })
      const items = Array.isArray(value?.items) ? value.items : []
      const nextOffset = typeof value?.nextOffset === 'number' ? value.nextOffset : null
      return { items, hasMore: value?.hasMore === true && nextOffset !== null, nextOffset }
    } catch (error) {
      if (!isMissingCommand(error)) throw error
      searchPageMissing = true
    }
  }
  if (offset) return { items: [], hasMore: false, nextOffset: null }
  return { items: await searchChats(query), hasMore: false, nextOffset: null }
}

// ---- 채팅방 고정·보관·되돌리기·영구 삭제(2026-11-16 김성균 개발자의 지시). 계약: archive-backend-port-notes.md "T38 채팅방 지우기".
//   chat_pin({chatId,pinned}) → {state: ok|not_found|storage_error}. chat_list·chat_search 항목에 pinned, pinnedOrder(고정한 차례, 풀면 null).
//   chat_archive({chatId}) → {state: ok|busy|not_found|storage_error}. 보관하면 목록과 찾기에서 빠진다(답하는 중이면 busy).
//   chat_archived_list() → [{id,title,groupId,updatedAt,archivedAt,provider}]
//   chat_restore({chatId}) → {state: ok|not_found|storage_error}. 고정 상태는 보관 전 그대로.
//   chat_delete({chatId}) → {state: deleted|busy|not_found|storage_error|provider_cleanup_failed, codexSession, claudeSession}
//     앱의 채팅방 말과 연결만 지운다(문서·원자료·작업 내역은 남는다). 답하는 중이면 busy.
//   이벤트 chat-list-updated {chatId}: 고정·보관·되돌리기·삭제가 되면.
// 명령이 없으면(isMissingCommand) 화면은 고정·보관 단추와 설정의 보관된 채팅방 쪽을 그리지 않는다. ----

export type ArchivedChat = { id: string; title: string; groupId: string | null; updatedAt: string; archivedAt: string | null; provider: AiProvider | null }
type KeepResult = { state?: string } | null

/** 되면 true. 아니면 뒤쪽이 준 state(busy 등)를 오류로 던진다(화면은 정해 둔 한 줄로 알린다). */
async function keep(command: string, args: Record<string, unknown>, done: string) {
  requireChats()
  const result = await invoke<KeepResult>(command, args)
  if (result?.state !== done) throw new Error(result?.state ?? 'failed')
}

export async function setChatPinned(chatId: string, pinned: boolean) {
  await keep('chat_pin', { chatId, pinned }, 'ok')
}

export async function archiveChat(chatId: string) {
  await keep('chat_archive', { chatId }, 'ok')
}

/** 보관한 채팅방 목록. */
export async function listArchivedChats() {
  requireChats()
  return invoke<ArchivedChat[]>('chat_archived_list')
}

/** 보관한 채팅방을 왼쪽 목록으로 되돌린다. */
export async function restoreChat(chatId: string) {
  await keep('chat_restore', { chatId }, 'ok')
}

/** 채팅방을 이 컴퓨터에서 영구 삭제한다(되돌릴 수 없다). 화면은 보관한 채팅방에서만 부른다. */
export async function deleteChat(chatId: string) {
  await keep('chat_delete', { chatId }, 'deleted')
}

/** 고정·보관·되돌리기·삭제가 된 뒤(다른 곳에서 바뀐 것도). */
export async function listenChatListUpdated(handler: () => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen('chat-list-updated', () => handler())
}

// ---- 보내기와 받기(archive-backend-port-notes.md 의 "대화 보내기" 절) ----
// chat_send 는 보낸 말을 저장하고 AI 를 부른 뒤 { status: 'started' } 를 바로 돌려준다.
// 그 AI 로는 보낼 수 없으면 말을 저장하지 않고 { status: 'unavailable' }. 답은 이벤트로 온다.

// 클로드가 연결되지 않았으면 { status: 'not_connected' }. 클로드 고속 모드를 지금 쓸 수 없다고 보내기 전에 확인되면 { status: 'fast_unavailable' }.
// 둘 다 말을 저장하지 않는다. 코덱스의 fast 는 그대로 전달된다(적용됐는지는 확인되지 않아 chat-done 의 fastState 가 unavailable).
// 고속 모드로 보낸 뒤 실패하면 백엔드가 그 말에 다시 보내기 표시를 남기고, 같은 글을 다시 보내면 그 말을 다시 쓴다(두 번 남지 않는다).
/**
 * chat_send 의 결과. 보내기가 시작되면 { status: 'started', userMessageId, handoffLimit }.
 * retryMessageId(답을 받지 못한 내 말의 id)를 주면 저장된 그 말(글·스킬·첨부)로 다시 보내고 새 말을 더하지 않는다.
 * 다시 보낼 수 없는 말이면 { status: 'retry_unavailable' } (archive-backend-port-notes.md T52·T55).
 */
export type SendResult = { status: 'started' | 'unavailable' | 'not_connected' | 'skill_unavailable' | 'fast_unavailable' | 'retry_unavailable' | string; userMessageId?: string | null }

/**
 * regenerateOf(2026-11-18 화면이 가정한 꼴, 셋째 줄에 부탁함): "다시 만들기"로 붙인 생성물의 원자료 id. 뒤쪽이 아직 받지 않으면 무시된다.
 */
export async function sendChatMessage(chatId: string, text: string, provider: AiProvider, model: string | null, effort: string | null, fast: boolean | null = null, skill: string | null = null, retryMessageId: string | null = null, regenerateOf: string | null = null) {
  requireChats()
  return invoke<SendResult>('chat_send', { chatId, text, provider, model, effort, ...(fast === null ? {} : { fast }), ...(skill === null ? {} : { skill }), ...(retryMessageId === null ? {} : { retryMessageId }), ...(regenerateOf === null ? {} : { regenerateOf }) })
}

/**
 * 어느 AI 로 보낼 수 있는지 미리 안다. 백엔드는 AI 를 먼저 보고, 보낼 수 없는 AI 면 아무것도 저장하지 않고
 * unavailable 을 준다. 보낼 수 있는 AI 는 빈 채팅방 번호라 저장에서 실패한다(아무것도 바뀌지 않는다).
 * chat_send 가 아예 없으면 null.
 */
export async function probeSendableProviders(providers: AiProvider[]): Promise<AiProvider[] | null> {
  if (!isDesktopRuntime()) return null
  const sendable: AiProvider[] = []
  for (const provider of providers) {
    try {
      const result = await invoke<SendResult>('chat_send', { chatId: '', text: '', provider, model: null, effort: null })
      if (result?.status !== 'unavailable') sendable.push(provider)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      if (/command\s+\S+\s+not found/i.test(message)) return null
      sendable.push(provider)
    }
  }
  return sendable
}

export async function cancelChatMessage(chatId: string) {
  requireChats()
  await invoke<null>('chat_cancel', { chatId })
}

/** 답이 오는 중에 조금씩 오는 글. */
export type ChatStreamEvent = { chatId: string; delta: string }
/**
 * 답이 끝났다. messageId 는 저장된 답의 말 번호.
 * notHandedOver: 다른 회사로 바꿔 보낼 때 전송 크기 제한으로 넘기지 못한 앞의 말의 수(0 이면 다 넘김).
 * fastState: 그 답의 실제 고속 상태 on / off / cooldown / null(모름).
 * 두 값은 이벤트 전에 그 답에 저장되므로, 화면은 chat_read 로 다시 읽은 말의 fast·notHandedOver 를 쓴다.
 */
export type ChatDoneEvent = { chatId: string; messageId?: string | null; message?: ChatMessage | null; notHandedOver?: number | boolean | null; fastState?: 'on' | 'off' | 'cooldown' | null }
/**
 * 실행 중 실패 또는 중단. 중단이면 reason 은 "요청을 중단했습니다".
 * status 가 fast_unavailable 이면 고속 모드를 지금 쓸 수 없어 답을 받지 못했다(보낸 말은 채팅방에 저장되어 있고, 같은 글을 다시 보내면 그 말을 다시 쓴다).
 * notHandedOver 도 오지만 오류일 때는 나머지 말이 처리됐다는 뜻이 아니고, 붙일 답도 없어 화면은 쓰지 않는다.
 */
/**
 * G18(archive-backend-port-notes.md "G18: AI 오류와 사용량 상태"): 까닭을 가른 값. kind 는 usageLimit, rateLimited, authentication, billing,
 * modelUnavailable, connectionClosed, timeout, toolMissing, serviceUnavailable, contextTooLong, unknown.
 * retryAt 은 회사가 준 다시 쓸 수 있는 때(주지 않으면 null, 클로드는 단위가 확인되지 않음). actions 는 retry, wait, signIn, checkApiKey,
 * checkBilling, switchModel, switchProvider, shorten(그리고 뒤쪽 코드의 install). providerCode 는 확인된 제한 코드만(화면에 보이지 않는다).
 */
export type ChatFailureInfo = { kind: string; retryAt?: number | string | null; actions?: string[] | null; providerCode?: string | null }
/** userMessageId: 답을 받지 못한 내 말의 id(다시 보내기에 쓴다). messageId 는 끊긴 답의 id 로 둘은 다르다. */
export type ChatErrorEvent = { chatId: string; reason?: string | null; message?: string | null; status?: string | null; notHandedOver?: number | null; messageId?: string | null; userMessageId?: string | null; error?: ChatFailureInfo | null }

export type ChatEventHandlers = {
  onStream: (event: ChatStreamEvent) => void
  onDone: (event: ChatDoneEvent) => void
  onError: (event: ChatErrorEvent) => void
  /** 채팅방이 찬 정도가 바뀜(chat-context-updated { chatId, context }). 회사를 바꾸면 unavailable 로 온다. */
  onContext?: (event: { chatId: string; context?: ChatContext | null }) => void
  /** 앞의 내용을 줄여 담음(chat-compacted { chatId, before, after }). */
  onCompacted?: (event: { chatId: string }) => void
  /** 첫 답 뒤에 클로드가 내용을 보고 채팅방에 짧은 이름을 지어 붙임(chat-renamed { chatId, title }). */
  onRenamed?: (event: { chatId: string; title: string }) => void
  /**
   * 회사가 스스로 다시 시도하는 중(2026-11-18 셋째 줄 계약 desktop-chat-failure-contract.md, 지금은 제미나이만):
   * chat-provider-retrying { chatId, provider, attempt, kind: rateLimited | serviceUnavailable | connectionClosed, retryAfterSeconds }.
   * 끝내 실패하면 chat-error 가 따로 온다.
   */
  onRetrying?: (event: ChatRetrying) => void
  /**
   * "다시 만들기"의 결과(2026-11-18 뒤쪽 6b21644 media_generation.rs, 계약 C52): chat-media-regeneration { chatId, userMessageId, status }.
   * status: recorded(새 판을 기록함) | not_detected(자동으로 찾지 못함, 생성물이 없다는 확정은 아님) | unavailable.
   */
  onMediaRegeneration?: (event: { chatId: string; status: string }) => void
}
export type ChatRetrying = { chatId: string; provider: string | null; attempt: number | null; kind: string | null; retryAfterSeconds: number | null }

export async function listenChatEvents(handlers: ChatEventHandlers) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<ChatStreamEvent>('chat-stream', (event) => handlers.onStream(event.payload)),
    listen<ChatDoneEvent>('chat-done', (event) => handlers.onDone(event.payload)),
    listen<ChatErrorEvent>('chat-error', (event) => handlers.onError(event.payload)),
    listen<{ chatId: string; context?: ChatContext | null }>('chat-context-updated', (event) => handlers.onContext?.(event.payload)),
    listen<{ chatId: string }>('chat-compacted', (event) => handlers.onCompacted?.(event.payload)),
    listen<{ chatId?: unknown; status?: unknown }>('chat-media-regeneration', (event) => {
      const p = event.payload ?? {}
      if (typeof p.chatId === 'string' && typeof p.status === 'string') handlers.onMediaRegeneration?.({ chatId: p.chatId, status: p.status })
    }),
    listen<{ chatId?: unknown; provider?: unknown; attempt?: unknown; kind?: unknown; retryAfterSeconds?: unknown }>('chat-provider-retrying', (event) => {
      const p = event.payload ?? {}
      if (typeof p.chatId !== 'string') return
      handlers.onRetrying?.({
        chatId: p.chatId,
        provider: typeof p.provider === 'string' ? p.provider : null,
        attempt: typeof p.attempt === 'number' && p.attempt > 0 ? p.attempt : null,
        kind: typeof p.kind === 'string' ? p.kind : null,
        retryAfterSeconds: typeof p.retryAfterSeconds === 'number' && p.retryAfterSeconds > 0 ? p.retryAfterSeconds : null,
      })
    }),
    listen<{ chatId: string; title?: string | null }>('chat-renamed', (event) => {
      const title = (event.payload?.title ?? '').trim()
      if (event.payload?.chatId && title) handlers.onRenamed?.({ chatId: event.payload.chatId, title })
    }),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 대화가 AI 가 한 번에 볼 수 있는 양을 얼마나 채웠는지. 알 수 없으면 state 가 unavailable. */
export type ChatContext =
  | {
      state: 'ok'
      usedTokens: number
      /** 한 번에 볼 수 있는 양과 찬 정도(세 회사 공통 값, cef940fa). 알 수 없으면 비어 온다. */
      limitTokens?: number | null
      usedPercent?: number | null
      provider?: AiProvider | null
      model?: string | null
      updatedAt?: string | null
    }
  | { state: 'unavailable'; reason?: string | null }

export async function readChatContext(chatId: string) {
  requireChats()
  return invoke<ChatContext>('chat_context', { chatId })
}

// ---- 채팅방 묶음(본부가 전한 계약: chat_group_list, chat_group_create, chat_group_rename, chat_group_reorder,
// chat_group_remove, chat_set_group, chat_list/chat_search 항목의 groupId). 백엔드가 아직 없다. ----

export type ChatGroup = { id: string; name: string; order: number; chatCount: number }

export async function listChatGroups() {
  requireChats()
  return invoke<ChatGroup[]>('chat_group_list')
}

export async function createChatGroup(name: string) {
  requireChats()
  return invoke<ChatGroup>('chat_group_create', { name })
}

export async function renameChatGroup(id: string, name: string) {
  requireChats()
  await invoke<null>('chat_group_rename', { id, name })
}

export async function reorderChatGroups(ids: string[]) {
  requireChats()
  await invoke<null>('chat_group_reorder', { ids })
}

export async function removeChatGroup(id: string) {
  requireChats()
  await invoke<null>('chat_group_remove', { id })
}

export async function setChatGroup(chatId: string, groupId: string | null) {
  requireChats()
  await invoke<null>('chat_set_group', { chatId, groupId })
}
