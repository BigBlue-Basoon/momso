// 채팅방 사본 쓰기의 형편(2026-11-17). 계약: apps/alpha/desktop/CHAT-ARCHIVE-RETRY.md.
//   chat-archive-changed { chatId, state: saved | unavailable, reason }: 실패 까닭 archiveMissing | notGitRepository | permissionDenied
//     | pathTooLong | diskError | redactionFailed | credentialStoreUnavailable | other. 본래 명령(가져오기 등)은 성공으로 끝나고 사본 실패만 따로 온다.
//   chat_archive_sync_status() → { state: saved | pending, pendingWrites, pendingDeletions }. 대기 중인 쓰기는 저장·업로드 전·앱 종료 때 다시 시도한다.
// 설정 "연결과 백업"의 GitHub 까닭 줄 자리에 알린다(채팅 화면에는 띄우지 않는다). 그 화면을 열기 전에 온 실패도 놓치지 않게 앱을 켤 때부터 듣는다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isDesktopRuntime } from './runtime'

export type ChatArchiveSync = { state: 'saved' | 'pending' | string; pendingWrites: number; pendingDeletions: number }

let lastFailure: string | null = null
const subs = new Set<() => void>()
let watching = false

export function watchChatArchive() {
  if (watching || !isDesktopRuntime()) return
  watching = true
  void listen<{ chatId?: string | null; state?: string | null; reason?: string | null }>('chat-archive-changed', (event) => {
    const state = event.payload?.state
    if (state === 'unavailable') lastFailure = event.payload?.reason || 'other'
    else if (state === 'saved') lastFailure = null
    else return
    subs.forEach((notify) => notify())
  }).catch(() => { watching = false })
}

/** 이 앱을 켠 뒤 마지막 사본 쓰기가 실패했으면 그 까닭(없으면 null). */
export function chatArchiveFailure() {
  return lastFailure
}

export function subscribeChatArchive(notify: () => void) {
  subs.add(notify)
  return () => { subs.delete(notify) }
}

export async function chatArchiveSyncStatus() {
  if (!isDesktopRuntime()) return null
  const value = await invoke<Partial<ChatArchiveSync> | null>('chat_archive_sync_status')
  if (!value || typeof value.state !== 'string') return null
  return { state: value.state, pendingWrites: Number(value.pendingWrites) || 0, pendingDeletions: Number(value.pendingDeletions) || 0 }
}
