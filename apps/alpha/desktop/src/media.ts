// 사진·동영상·소리(미디어 화면). 계약: archive-backend-port-notes.md "2026-11-16 옮긴 것"
//   media_list({ kind, before, limit }) → [{ id, kind: 'image' | 'video' | 'audio', title, createdAt, width, height, durationSec,
//                                            path, posterPath, chatId, by, how, at, chats: [{ id, title }] }]
//   - path: 파일의 로컬 경로(화면은 convertFileSrc 로 보인다). posterPath: 동영상의 첫 장면 그림(없으면 null).
//   - createdAt: 미디어를 등록한 때. 화면은 새것부터 날짜별로 묶는다.
//   - by·how·at: 첨부 작업 내역이 있으면 { kind: 'user' }·'attached'·그때. AI 가 채팅방에서 만들고 고친 것은 새 기록부터
//     { kind: 'ai', provider, model }·'created' | 'edited'·그때. 알 수 없으면 null(화면은 그 칸을 비운다. by 가 null 이면 누가 했다고 적지 않는다).
//   - evidence: 'tool'(도구가 성공을 알림) | 'diff'(파일 전후 비교로 확인) | null(옛 기록). 값만 받고 화면에는 보이지 않는다(보일지는 그가 정하지 않음).
//   - 겹쳐 답한 채팅방이 있어 누가 했는지 가를 수 없으면 by 가 null 이다.
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { isDesktopRuntime } from './runtime'

export type MediaItem = {
  id: string
  kind: 'image' | 'video' | 'audio'
  title: string | null
  createdAt: string
  width: number | null
  height: number | null
  durationSec: number | null
  path: string
  posterPath: string | null
  chatId: string | null
  chats?: Array<{ id: string; title: string | null }> | null
  by?: { kind: 'ai'; provider: 'codex' | 'claude' | 'gemini'; model?: string | null } | { kind: 'user' } | null
  how?: 'created' | 'edited' | 'attached' | null
  /** 그 일을 한 때(모르면 null). */
  at?: string | null
  evidence?: 'tool' | 'diff' | null
}

/** 그 미디어를 만든(붙인) 채팅방 하나. chats 가 있으면 첫째, 없으면 예전 chatId. */
export function mediaChat(item: MediaItem): { id: string; title: string | null } | null {
  if (item.chats && item.chats.length > 0) return item.chats[0]
  return item.chatId ? { id: item.chatId, title: null } : null
}

export async function listMedia() {
  if (!isDesktopRuntime()) throw new Error('media_list needs the desktop runtime')
  return invoke<MediaItem[]>('media_list')
}

/** 화면에 보일 주소. 로컬 경로는 Tauri 의 주소로 바꾸고, 이미 주소(data:, http:)면 그대로. */
export function mediaSrc(path: string | null) {
  if (!path) return null
  return /^(data:|https?:|blob:)/.test(path) ? path : convertFileSrc(path)
}
