// 받아 오기와 되살리기의 진행(2026-11-17). 계약: desktop-github-b29-errors.md "첫 수신 진행 이벤트", apps/alpha/desktop/RESTORE-PROGRESS.md.
//   archive-first-receive-progress { completed, total, state, available }: 새 PC 에서 GitHub 를 처음 받거나 끊긴 뒤 다시 받을 때(수동 받기에서만).
//     total 은 파일 수·바이트가 아니라 세 단계. state: recovering | receiving | indexing | restoring | complete | empty | interrupted | failed.
//   chat-archive-restore-progress { completed, total, state, available }: 받은 채팅 사본을 앱에 되살릴 때. completed/total 은 사본 수.
//     state: restoring | complete | failed. 진행 중에는 available 이 false 이고, 끝나야(complete) 새로 받은 채팅방을 쓸 수 있다.
//     그동안 화면은 채팅방을 바꾸는 요청을 보내지 않는다(보내기, 새 채팅방, 이름 바꾸기·고정·보관·삭제·묶음). 실패하면 이 PC 에 있던 채팅방은 그대로다.
import { listen } from '@tauri-apps/api/event'
import { isDesktopRuntime } from './runtime'

export type RestoreProgress = { completed: number; total: number; state: string; available: boolean }

function read(payload: Partial<RestoreProgress> | null | undefined): RestoreProgress | null {
  if (!payload || typeof payload.state !== 'string') return null
  return {
    completed: typeof payload.completed === 'number' ? payload.completed : 0,
    total: typeof payload.total === 'number' ? payload.total : 0,
    state: payload.state,
    available: payload.available === true,
  }
}

export async function listenFirstReceiveProgress(onProgress: (value: RestoreProgress) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<Partial<RestoreProgress>>('archive-first-receive-progress', (event) => { const value = read(event.payload); if (value) onProgress(value) })
}

export async function listenChatRestoreProgress(onProgress: (value: RestoreProgress) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<Partial<RestoreProgress>>('chat-archive-restore-progress', (event) => { const value = read(event.payload); if (value) onProgress(value) })
}

/** 진행 막대의 너비(0~100). 전체를 모르면 null(막대를 그리지 않는다). */
export function progressPercent(value: RestoreProgress) {
  if (!(value.total > 0)) return null
  return Math.max(4, Math.min(100, (value.completed / value.total) * 100))
}

// ---- 처음 받아 오는 동안(앱 전체). 받는 동안은 뒤쪽이 문서 쓰기도 거절하므로(2026-11-17 본부 결정) 화면도 문서 고치기·되돌리기를 막는다. ----
let receiving: RestoreProgress | null = null
const receivingSubs = new Set<() => void>()
let receiveWatch: Promise<() => void> | null = null

/** 앱을 켤 때 한 번 부른다(두 번 불러도 하나만 듣는다). */
export function watchFirstReceive() {
  if (receiveWatch) return
  receiveWatch = listenFirstReceiveProgress((value) => {
    receiving = !value.available && ['recovering', 'receiving', 'indexing', 'restoring'].includes(value.state) ? value : null
    receivingSubs.forEach((notify) => notify())
  })
}

/** 지금 처음 받아 오는 중인가. */
export function isFirstReceiving() {
  return receiving !== null
}

export function subscribeFirstReceive(notify: () => void) {
  receivingSubs.add(notify)
  return () => { receivingSubs.delete(notify) }
}
