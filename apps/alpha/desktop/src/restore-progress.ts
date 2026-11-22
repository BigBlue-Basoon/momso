// 받아 오기와 되살리기의 진행(2026-11-17). 계약: desktop-github-b29-errors.md "첫 수신 진행 이벤트", apps/alpha/desktop/RESTORE-PROGRESS.md.
//   archive-first-receive-progress { completed, total, state, available }: 새 PC 에서 GitHub 를 처음 받거나 끊긴 뒤 다시 받을 때(수동 받기에서만).
//     total 은 파일 수·바이트가 아니라 세 단계. state: recovering | receiving | indexing | restoring | complete | empty | interrupted | failed.
//   chat-archive-restore-progress { completed, total, state, available }: 받은 채팅 사본을 앱에 되살릴 때. completed/total 은 사본 수.
//     state: restoring | complete | failed. 진행 중에는 available 이 false 이고, 끝나야(complete) 새로 받은 채팅방을 쓸 수 있다.
//     그동안 화면은 채팅방을 바꾸는 요청을 보내지 않는다(보내기, 새 채팅방, 이름 바꾸기·고정·보관·삭제·묶음). 실패하면 이 PC 에 있던 채팅방은 그대로다.
import { invoke, listen } from './host'
import { isDesktopRuntime } from './runtime'

// 2026-11-17 뒤쪽 fdce0939: 처음 받아 오기 값에 reason(first_receive_conflict 등)과 conflictPaths(같은 이름이 있어 멈춘 파일)가 더해졌다.
//   archive_first_receive_status() 는 지금 형편을 같은 꼴로 준다(앱을 다시 켜 스스로 이어받을 때 이벤트를 놓치지 않게 화면이 뜨자마자 묻는다).
export type RestoreProgress = { completed: number; total: number; state: string; available: boolean; reason?: string | null; conflictPaths?: string[] }

function read(payload: Partial<RestoreProgress> | null | undefined): RestoreProgress | null {
  if (!payload || typeof payload.state !== 'string') return null
  return {
    completed: typeof payload.completed === 'number' ? payload.completed : 0,
    total: typeof payload.total === 'number' ? payload.total : 0,
    state: payload.state,
    available: payload.available === true,
    reason: typeof payload.reason === 'string' ? payload.reason : null,
    conflictPaths: Array.isArray(payload.conflictPaths) ? payload.conflictPaths.filter((path): path is string => typeof path === 'string') : [],
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
const RECEIVING_STATES = ['recovering', 'receiving', 'indexing', 'restoring']
/** 마지막으로 안 형편: 받는 중(receiving), 같은 이름의 파일 때문에 멈춤(conflictPaths), 받아 올 것이 없음(empty). 바뀔 때만 새 물건(useSyncExternalStore 용). */
export type FirstReceiveSnapshot = { receiving: RestoreProgress | null; conflictPaths: string[]; empty: boolean }
let snapshot: FirstReceiveSnapshot = { receiving: null, conflictPaths: [], empty: false }
let receiving: RestoreProgress | null = null
const receivingSubs = new Set<() => void>()
let receiveWatch: Promise<() => void> | null = null

function apply(value: RestoreProgress) {
  receiving = !value.available && RECEIVING_STATES.includes(value.state) ? value : null
  const conflictPaths = value.reason === 'first_receive_conflict' ? value.conflictPaths ?? [] : []
  snapshot = { receiving, conflictPaths, empty: value.state === 'empty' }
  receivingSubs.forEach((notify) => notify())
}

/** 앱을 켤 때 한 번 부른다(두 번 불러도 하나만 듣는다). 먼저 지금 형편을 묻고, 그 뒤 이벤트를 듣는다. */
export function watchFirstReceive() {
  if (receiveWatch || !isDesktopRuntime()) return
  receiveWatch = listenFirstReceiveProgress(apply)
  invoke<Partial<RestoreProgress> | null>('archive_first_receive_status')
    .then((value) => { const progress = read(value); if (progress) apply(progress) })
    .catch(() => {})
}

/** 지금 처음 받아 오는 중인가. */
export function isFirstReceiving() {
  return receiving !== null
}

export function firstReceiveSnapshot() {
  return snapshot
}

/** 뒤쪽이 받는 동안이라 문서 쓰기를 거절한 오류인가(first_receive_in_progress). */
export function isFirstReceiveRefusal(error: unknown) {
  return String(error instanceof Error ? error.message : error).includes('first_receive_in_progress')
}

export function subscribeFirstReceive(notify: () => void) {
  receivingSubs.add(notify)
  return () => { receivingSubs.delete(notify) }
}

// ---- 앱 시작 진행(2026-11-17 뒤쪽 fdce0939). runtime_startup_status() 와 이벤트 runtime-startup-progress: { state, completed, total(4), available }.
//   state: checking | backup | migrating | storageReady | complete. DB 꼴이 바뀌는 날에는 채팅방이 많으면 몇 초 걸린다(1,000개에 7초).
//   화면이 뜨자마자 한 번 묻고 그 뒤 이벤트를 듣는다. 새 화면 없이 지금의 "채팅방 목록 불러오는 중" 줄에 단계만 얹는다.
export type StartupProgress = { state: string; completed: number; total: number; available: boolean }

function readStartup(value: Partial<StartupProgress> | null | undefined): StartupProgress | null {
  if (!value || typeof value.state !== 'string') return null
  return {
    state: value.state,
    completed: typeof value.completed === 'number' ? value.completed : 0,
    total: typeof value.total === 'number' ? value.total : 0,
    available: value.available === true,
  }
}

/** 시작 진행을 듣고 지금 형편도 한 번 묻는다. 정리 함수를 돌려준다. */
export function watchStartup(onProgress: (value: StartupProgress) => void) {
  if (!isDesktopRuntime()) return () => {}
  let stop: (() => void) | undefined
  let disposed = false
  void listen<Partial<StartupProgress>>('runtime-startup-progress', (event) => { const value = readStartup(event.payload); if (value) onProgress(value) })
    .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten })
    .catch(() => {})
  invoke<Partial<StartupProgress> | null>('runtime_startup_status')
    .then((value) => { const progress = readStartup(value); if (progress && !disposed) onProgress(progress) })
    .catch(() => {})
  return () => { disposed = true; stop?.() }
}
