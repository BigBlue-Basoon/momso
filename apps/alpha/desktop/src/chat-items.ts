// 한 채팅방에서 다룬 문서와 미디어(오른쪽 패널). 계약: archive-backend-port-notes.md "2026-11-16 옮긴 것 › 채팅방에서 다룬 문서와 미디어"
//   chat_items({ chatId }) → { documents: [{ id, title, summary, kind, status, updatedAt, how, by, at }],
//                              media: [{ id, kind, title, createdAt, how, by, at, path }] }
//   how: 문서 'created' | 'edited' | 'read', 미디어 'created' | 'edited' | 'attached'. 새 기록부터 채워진다(AI 답 전후 파일 비교). 옛 기록은 null.
//   by: { kind: 'ai', provider, model(없으면 null) } | { kind: 'user' } | null. 겹쳐 답한 채팅방이 있어 누가 했는지 가를 수 없으면 null. at 도 null 일 수 있다.
//   evidence: 'tool'(도구가 성공을 알림) | 'diff'(파일 전후 비교로 확인) | null(옛 기록). 값만 받고 화면에는 보이지 않는다(보일지는 그가 정하지 않음).
//   null 인 칸은 화면이 비워 둔다(짐작해 채우지 않는다. by 가 null 이면 누가 했다고 적지 않는다).
//   이벤트 chat-items-updated { chatId }: 첨부, 채팅방 번호가 든 작업 내역, AI 답 완료 때.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AiProvider } from './ai-models'
import type { MediaItem } from './media'
import { isDesktopRuntime } from './runtime'

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

export type ChatItems = { documents: ChatDocumentItem[]; media: MediaItem[] }

export async function getChatItems(chatId: string) {
  if (!isDesktopRuntime()) throw new Error('chat_items needs the desktop runtime')
  const value = await invoke<Partial<ChatItems> | null>('chat_items', { chatId })
  return { documents: value?.documents ?? [], media: value?.media ?? [] } as ChatItems
}

export async function listenChatItems(onChange: (chatId: string) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ chatId?: string | null }>('chat-items-updated', (event) => {
    if (event.payload?.chatId) onChange(event.payload.chatId)
  })
}

export function chatItemCount(items: ChatItems | null) {
  return items ? items.documents.length + items.media.length : 0
}
