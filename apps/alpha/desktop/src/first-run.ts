// 첫 실행. 방향 문서 22줄(2026-11-14): 처음 실행하면 셋 중 최소 하나는 연결해야 앱이 시작된다("건너뛰기 절대 못해 최소 하나는 해야해").
// 2026-11-16 김성균 개발자: 영어 username 을 처음 실행할 때 받는다(GitHub 저장소와 원자료 버킷 이름 momso-username 에 쓴다).
// 계약(뒤쪽 archive_remote.rs):
//   momso_username_get() → { state: 'ok' | 'unavailable', username: string | null }
//   momso_username_set({ username }) → { state: 'ok' | 'invalid_username' | 'unavailable', username }
//   받는 글자: 영문자·숫자·- _ . (94자까지, .git·.wiki 로 끝나면 안 됨)
import { invoke } from '@tauri-apps/api/core'
import { getProviderAuth, isConnected } from './provider-auth'

export type UsernameResult = { state: string; username: string | null }

export async function getUsername() { return invoke<UsernameResult>('momso_username_get') }
export async function setUsername(username: string) { return invoke<UsernameResult>('momso_username_set', { username }) }

/** 뒤쪽과 같은 규칙. 'chars' 는 쓸 수 없는 글자가 들어간 것, 'other' 는 길이·끝말이 맞지 않는 것. */
export function checkUsername(value: string): 'ok' | 'empty' | 'chars' | 'other' {
  if (!value) return 'empty'
  if (!/^[A-Za-z0-9._-]+$/.test(value)) return 'chars'
  const lower = value.toLowerCase()
  if (value.length > 94 || lower.endsWith('.git') || lower.endsWith('.wiki')) return 'other'
  return 'ok'
}

// 첫 실행을 마친 것을 이 컴퓨터에 적어 둔다. 다음부터는 AI 상태를 기다리지 않고 바로 작업 화면을 연다.
const DONE_KEY = 'momso.firstRunDone'
export function markFirstRunDone() {
  try { localStorage.setItem(DONE_KEY, '1') } catch { /* 적지 못해도 다음 실행에서 다시 확인한다 */ }
}
function firstRunMarked() {
  try { return localStorage.getItem(DONE_KEY) === '1' } catch { return false }
}

export type FirstRunCheck = {
  /** 첫 실행 화면을 보일지. */
  show: boolean
  /** 저장된 username. 이름 명령이 없거나 읽지 못하면 null 이고, 그때는 이름 단계를 두지 않는다. */
  username: string | null
  nameStep: boolean
}

/**
 * 이미 쓰던 사람(username 이 있고, 도구가 깔린 채 연결된 AI 가 있는 사람)에게는 첫 실행 화면을 다시 보이지 않는다.
 * AI 상태를 읽지 못하면 연결을 확인할 수 없으니 첫 실행 화면을 보인다(그 화면에서 다시 읽기를 할 수 있다. 뒤쪽 노트 293줄).
 */
export async function checkFirstRun(): Promise<FirstRunCheck> {
  let username: string | null = null
  let nameStep = false
  try {
    const result = await getUsername()
    if (result.state === 'ok') {
      username = result.username || null
      nameStep = true
    }
  } catch { /* 이름 명령이 없다 */ }
  if (username && firstRunMarked()) return { show: false, username, nameStep }
  let connected = false
  try {
    connected = (await getProviderAuth()).some(isConnected)
  } catch {
    return { show: true, username, nameStep }
  }
  const needsName = nameStep && !username
  if (connected && !needsName) {
    markFirstRunDone()
    return { show: false, username, nameStep }
  }
  return { show: true, username, nameStep }
}
