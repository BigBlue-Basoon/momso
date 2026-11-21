// 저장된 로그인(2026-11-21 뒤쪽 275e017, 계약: 통합 가지 docs/development/desktop-saved-logins-s7-design.md "화면 계약 초안").
//   saved_login_list() → [{ id, origin, label, username, askBeforeUse, loginPath, defaultForPath }]
//   saved_login_set({ input: { id?, origin, label, username, password?, askBeforeUse?, loginPath?, defaultForPath? } }) → 같은 모양
//     새 항목은 비밀번호가 꼭 있어야 하고, askBeforeUse 를 빼면 자동 사용(false). 고칠 때 뺀 설정·경로는 그대로, loginPath '' 는 경로 지우기,
//     비밀번호를 빼면 기존 것을 그대로 쓴다. 사이트 주소는 고칠 수 없다(origin_change_requires_new_login: 삭제한 뒤 새로 저장).
//     origin 은 https://호스트[:포트] 만 받는다(경로·물음표·#·아이디 붙은 주소는 invalid_origin).
//   saved_login_delete({ id }) → 빈 값. 목록·바꾸기 명령은 몸소 주 화면에서만 부를 수 있다.
//   이벤트 saved-login-selection-request { requestId, chatId, entries } → saved_login_select({ requestId, loginId })
//     뒤쪽이 같은 로그인 화면의 계정 여럿 중 하나를 고르지 못할 때. 답이 없으면 300초 뒤(또는 답을 멈추면) 뒤쪽이 혼자 닫는다.
//     그만두기는 saved_login_cancel({ requestId })(AI 에는 cancelled). 닫힌 뒤 고르거나 그만두면 unavailable.
//   이벤트 saved-login-selection-closed { requestId, reason }(2026-11-21 뒤쪽 e8dab5e): 고름·그만둠·답 멈춤·300초 지남에 한 번.
//     reason 은 selected, cancelled(그만둠 또는 답 멈춤), timeout.
//   이벤트 saved-login-used { chatId, id, origin }: 로그인이 끝난 것이 확인됨(화면은 아직 쓰지 않는다).
//   오류 값: capacity_exceeded(저장 항목 전체 2,200바이트 넘음), credential_store_unavailable, invalid_origin, invalid_input,
//     duplicate_label(같은 사이트에 같은 이름), origin_change_requires_new_login, default_conflict, not_found, unavailable.
// 비밀번호는 목록·이벤트 어디에도 오지 않는다. 화면은 비밀번호를 상태·기록(console)·오류 글에 남기지 않는다(보낸 뒤 칸을 비운다).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isDesktopRuntime } from './runtime'

export type SavedLogin = {
  id: string
  /** https://호스트[:포트] */
  origin: string
  /** 사람이 붙인 이름(같은 사이트 안에서 겹치지 않는다). */
  label: string
  username: string
  /** true 면 쓸 때마다 승인 카드로 묻는다. 기본은 false(묻지 않고 씀). */
  askBeforeUse: boolean
  /** 이 로그인을 쓸 로그인 화면의 경로("/admin/login"). 없으면 null. */
  loginPath: string | null
  /** 같은 사이트·경로에 계정이 여럿일 때의 기본 계정. */
  defaultForPath: boolean
}

export type SavedLoginInput = {
  id?: string
  origin: string
  label: string
  username: string
  password?: string
  askBeforeUse?: boolean
  loginPath?: string
  defaultForPath?: boolean
}

export type LoginChoiceRequest = { requestId: string; chatId: string | null; entries: SavedLogin[] }

function readLogin(value: unknown): SavedLogin | null {
  if (!value || typeof value !== 'object') return null
  const item = value as Record<string, unknown>
  if (typeof item.id !== 'string' || typeof item.origin !== 'string') return null
  return {
    id: item.id,
    origin: item.origin,
    label: typeof item.label === 'string' ? item.label : '',
    username: typeof item.username === 'string' ? item.username : '',
    askBeforeUse: item.askBeforeUse === true,
    loginPath: typeof item.loginPath === 'string' && item.loginPath ? item.loginPath : null,
    defaultForPath: item.defaultForPath === true,
  }
}

function readLogins(value: unknown) {
  return Array.isArray(value) ? value.flatMap((entry) => { const login = readLogin(entry); return login ? [login] : [] }) : []
}

export async function listSavedLogins() {
  return readLogins(await invoke<unknown>('saved_login_list'))
}

/** 넣기·고치기. input 의 비밀번호는 넘기기만 하고 어디에도 담지 않는다. */
export async function saveSavedLogin(input: SavedLoginInput) {
  return readLogin(await invoke<unknown>('saved_login_set', { input }))
}

export function deleteSavedLogin(id: string) {
  return invoke<void>('saved_login_delete', { id })
}

export function selectSavedLogin(requestId: string, loginId: string) {
  return invoke<void>('saved_login_select', { requestId, loginId })
}

export function cancelLoginChoice(requestId: string) {
  return invoke<void>('saved_login_cancel', { requestId })
}

const ERRORS = [
  'capacity_exceeded',
  'credential_store_unavailable',
  'origin_change_requires_new_login',
  'invalid_origin',
  'invalid_input',
  'duplicate_label',
  'default_conflict',
  'not_found',
  'unavailable',
] as const
export type SavedLoginError = (typeof ERRORS)[number]

/** 뒤쪽이 준 오류 값(정해진 값만 본다. 다른 글은 화면에 옮기지 않는다). */
export function savedLoginErrorCode(error: unknown): SavedLoginError | null {
  const text = typeof error === 'string' ? error : String((error as { message?: unknown })?.message ?? '')
  return ERRORS.find((code) => text === code) ?? ERRORS.find((code) => text.includes(code)) ?? null
}

/**
 * 사람이 적은 사이트 주소를 뒤쪽이 받는 꼴(https://호스트[:포트])로. 앞에 https:// 가 없으면 붙이고, 로그인 화면 주소를 통째로 붙여 넣었으면
 * 경로 이하는 뺀다. 읽지 못하는 글이면 적은 그대로 넘긴다(뒤쪽이 invalid_origin 으로 돌려준다).
 */
export function siteOrigin(text: string) {
  const value = text.trim()
  if (!value) return ''
  try {
    const url = new URL(value.includes('://') ? value : `https://${value}`)
    return url.protocol === 'https:' && url.host && !url.username && !url.password ? url.origin : value
  } catch {
    return value
  }
}

/** 목록·카드에 보일 사이트: 호스트(포트가 있으면 함께). */
export function siteHost(origin: string) {
  try { return new URL(origin).host || origin } catch { return origin }
}

export async function listenLoginChoices(onRequest: (request: LoginChoiceRequest) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ requestId?: unknown; chatId?: unknown; entries?: unknown }>('saved-login-selection-request', (event) => {
    const p = event.payload ?? {}
    if (typeof p.requestId !== 'string' || !p.requestId) return
    const entries = readLogins(p.entries)
    if (entries.length === 0) return
    onRequest({ requestId: p.requestId, chatId: typeof p.chatId === 'string' && p.chatId ? p.chatId : null, entries })
  })
}

export type LoginChoiceClosedReason = 'selected' | 'cancelled' | 'timeout'

/** 고르기 물음이 닫힘(saved-login-selection-closed). 모르는 까닭 값이면 null. */
export async function listenLoginChoicesClosed(onClosed: (requestId: string, reason: LoginChoiceClosedReason | null) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ requestId?: unknown; reason?: unknown }>('saved-login-selection-closed', (event) => {
    const p = event.payload ?? {}
    if (typeof p.requestId !== 'string' || !p.requestId) return
    const reason = p.reason === 'selected' || p.reason === 'cancelled' || p.reason === 'timeout' ? p.reason : null
    onClosed(p.requestId, reason)
  })
}
