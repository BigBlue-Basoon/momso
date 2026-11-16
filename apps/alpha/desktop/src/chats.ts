// 대화(왼쪽 대화 목록, 가운데 대화 칸) 명령을 부르는 얇은 층.
// 모양은 본부가 전한 계약을 따른다(chat_list, chat_create, chat_read, chat_append, chat_rename, chat_search).
// 백엔드가 들어오면 momso-public-prep/archive-backend-port-notes.md 의 대화 절과 견준다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AiProvider } from './ai-models'
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
  /** 고정한 채팅방인지(고정 명령이 들어오면 chat_list 가 준다고 짐작). 고정한 것은 목록 맨 위에 모인다. */
  pinned?: boolean | null
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
  /** 이 답을 쓸 때 다른 회사에 넘기지 못한 앞의 말 수(chat_read). 옛 말은 null. 1 이상일 때만 알린다. */
  notHandedOver?: number | null
  /** 사용자가 이 말에 고른 스킬의 부르는 이름(스킬 계약). */
  skill?: string | null
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

export async function createChat() {
  requireChats()
  return invoke<ChatEntry>('chat_create')
}

export async function readChat(id: string) {
  requireChats()
  return invoke<ChatDocument>('chat_read', { id })
}

/** 말 붙이기. AI 의 답이면 답한 곳과 모델을 함께 적는다(없으면 null). */
export async function appendChatMessage(id: string, role: ChatRole, text: string, provider: AiProvider | null = null, model: string | null = null) {
  requireChats()
  return invoke<ChatMessage>('chat_append', { id, role, text, provider, model })
}

export async function renameChat(id: string, title: string) {
  requireChats()
  await invoke<null>('chat_rename', { id, title })
}

export async function searchChats(query: string) {
  requireChats()
  return invoke<ChatSearchResult[]>('chat_search', { query })
}

// ---- 채팅방 고정·보관·되돌리기·영구 삭제(2026-11-16 김성균 개발자의 지시). 뒤쪽 명령은 첫째 줄이 만드는 중이라
// 이름과 모양은 짐작이다(본부가 계약을 알리면 맞춘다). 보관한 채팅방은 chat_list 에 오지 않고 chat_archived_list 에만 온다고 본다.
// 명령이 없으면(isMissingCommand) 화면은 고정·보관 단추와 설정의 보관된 채팅방 쪽을 그리지 않는다. ----

export async function setChatPinned(chatId: string, pinned: boolean) {
  requireChats()
  await invoke<null>('chat_pin', { chatId, pinned })
}

export async function archiveChat(chatId: string) {
  requireChats()
  await invoke<null>('chat_archive', { chatId })
}

/** 보관한 채팅방 목록(최근에 보관한 것이 먼저라고 짐작). */
export async function listArchivedChats() {
  requireChats()
  return invoke<ChatEntry[]>('chat_archived_list')
}

/** 보관한 채팅방을 왼쪽 목록으로 되돌린다. */
export async function unarchiveChat(chatId: string) {
  requireChats()
  await invoke<null>('chat_unarchive', { chatId })
}

/** 보관한 채팅방을 이 컴퓨터에서 영구 삭제한다(되돌릴 수 없다). */
export async function deleteChat(chatId: string) {
  requireChats()
  await invoke<null>('chat_delete', { chatId })
}

// ---- 보내기와 받기(archive-backend-port-notes.md 의 "대화 보내기" 절) ----
// chat_send 는 보낸 말을 저장하고 AI 를 부른 뒤 { status: 'started' } 를 바로 돌려준다.
// 그 AI 로는 보낼 수 없으면 말을 저장하지 않고 { status: 'unavailable' }. 답은 이벤트로 온다.

// 클로드가 연결되지 않았으면 { status: 'not_connected' }. 클로드 고속 모드를 지금 쓸 수 없다고 보내기 전에 확인되면 { status: 'fast_unavailable' }.
// 둘 다 말을 저장하지 않는다. 코덱스의 fast 는 그대로 전달된다(적용됐는지는 확인되지 않아 chat-done 의 fastState 가 unavailable).
// 고속 모드로 보낸 뒤 실패하면 백엔드가 그 말에 다시 보내기 표시를 남기고, 같은 글을 다시 보내면 그 말을 다시 쓴다(두 번 남지 않는다).
export type SendResult = { status: 'started' | 'unavailable' | 'not_connected' | 'skill_unavailable' | 'fast_unavailable' | string }

export async function sendChatMessage(chatId: string, text: string, provider: AiProvider, model: string | null, effort: string | null, fast: boolean | null = null, skill: string | null = null) {
  requireChats()
  return invoke<SendResult>('chat_send', { chatId, text, provider, model, effort, ...(fast === null ? {} : { fast }), ...(skill === null ? {} : { skill }) })
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
 * fastState: 고속 모드로 보냈을 때만 온다. on 이면 고속 모드로 답했고, unavailable 이면 확인되지 않았다.
 * 두 값은 이벤트 전에 그 답에 저장되므로, 화면은 chat_read 로 다시 읽은 말의 fast·notHandedOver 를 쓴다.
 */
export type ChatDoneEvent = { chatId: string; messageId?: string | null; message?: ChatMessage | null; notHandedOver?: number | boolean | null; fastState?: 'on' | 'unavailable' | null }
/**
 * 실행 중 실패 또는 중단. 중단이면 reason 은 "요청을 중단했습니다".
 * status 가 fast_unavailable 이면 고속 모드를 지금 쓸 수 없어 답을 받지 못했다(보낸 말은 채팅방에 저장되어 있고, 같은 글을 다시 보내면 그 말을 다시 쓴다).
 * notHandedOver 도 오지만 오류일 때는 나머지 말이 처리됐다는 뜻이 아니고, 붙일 답도 없어 화면은 쓰지 않는다.
 */
export type ChatErrorEvent = { chatId: string; reason?: string | null; message?: string | null; status?: string | null; notHandedOver?: number | null }

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
}

export async function listenChatEvents(handlers: ChatEventHandlers) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<ChatStreamEvent>('chat-stream', (event) => handlers.onStream(event.payload)),
    listen<ChatDoneEvent>('chat-done', (event) => handlers.onDone(event.payload)),
    listen<ChatErrorEvent>('chat-error', (event) => handlers.onError(event.payload)),
    listen<{ chatId: string; context?: ChatContext | null }>('chat-context-updated', (event) => handlers.onContext?.(event.payload)),
    listen<{ chatId: string }>('chat-compacted', (event) => handlers.onCompacted?.(event.payload)),
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
      /** 한 번에 볼 수 있는 양(새 계약 limitTokens, 예전 짐작 maxTokens). */
      limitTokens?: number | null
      maxTokens?: number | null
      usedPercent?: number | null
      /** 앞의 내용을 줄여 담기 시작하는 찬 정도(%). 오면 "줄이기까지 남은 양"을 보인다. */
      compactAtPercent?: number | null
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
