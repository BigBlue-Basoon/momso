// 설정의 AI 로그인. 계약: momso-desktop-backend/outputs/2026-11-16-momso-provider-login-contract.md
//   provider_auth_status() → [{ provider, installed, runnable, state: signedOut|account|apiKey|unavailable, accountLabel, planLabel, apiKeySaved, keyVerification, methods: ('account'|'apiKey')[], active }]
//     keyVerification: notConfigured | verifying | verified | rejected | unavailable | notSupported (2026-11-17 desktop-screen-backend-contract.md "AI 연결 확인")
//     API 키를 연결됨으로 칠 조건(계약 한 줄): runnable && apiKeySaved && keyVerification 이 verified 또는 notSupported.
//   provider_first_run_eligible() → boolean. 첫 실행의 "시작하기"는 이 값 하나로 정한다(화면이 조건을 다시 짜지 않는다).
//   provider_api_key_verify({ provider }) → 'verifying' | 'notConfigured' | 'unavailable' | 'notSupported'. 결과는 provider-auth-changed 와 상태 조회로.
//   provider_login_start({ provider }) → 'inProgress' | 'unavailable'
//   provider_login_cancel({ provider }) → 'cancelled' | 'notInProgress' | 'unavailable'
//   provider_logout({ provider }) → 'signedOut' | 'inProgress' | 'unavailable'
//   provider_api_key_set({ provider, apiKey }) → 'verifying'(저장한 뒤 확인을 시작함. 예전 뒤쪽은 'keySaved') | 'invalidKey' | 'unavailable'
//   provider_api_key_clear({ provider }) → 'keyCleared' | 'unavailable'
//   install_external_provider({ provider }) → 'installed' | 'installFailed' | 'unavailable' | 'inProgress'(다른 설치 중이면 완료 이벤트를 기다린다). 끝나면 상태를 다시 읽는다.
//   이벤트 provider-install-progress { provider, phase: installing|verifying|completed|failed, receivedBytes, totalBytes(모르면 null), canCancel(false), reason }
//     verifying 은 설치한 도구가 실행되는지 확인하는 진행 단계다(실패가 아니다, 2026-11-17 뒤쪽 clean-windows-first-run-check.md).
//     reason(실패 때, 확인된 경우): runtimeMissing | timeout | permissionDenied | startFailed | toolNotFound | toolNotRunnable | checksumMismatch | diskFull | installFailed | unavailable
//   provider_auth_status 의 state 'apiKey'(apiKeySaved)는 키를 저장했다는 뜻이고, 키가 맞는지 확인했다는 뜻이 아니다(2026-11-17 계약).
//   이벤트 provider-auth-changed { provider, state } (받으면 상태를 다시 읽는다)
// API 키는 받아서 바로 넘기기만 한다. 화면은 키를 어디에도 남기지 않는다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AiProvider } from './ai-models'
import { isDesktopRuntime } from './runtime'

export type KeyVerification = 'notConfigured' | 'verifying' | 'verified' | 'rejected' | 'unavailable' | 'notSupported'

export type ProviderAuth = {
  provider: AiProvider
  installed: boolean
  runnable?: boolean
  state: 'signedOut' | 'account' | 'apiKey' | 'unavailable'
  accountLabel: string | null
  planLabel: string | null
  apiKeySaved: boolean
  keyVerification?: KeyVerification | null
  methods: Array<'account' | 'apiKey'>
  active: string | null
}

function need() {
  if (!isDesktopRuntime()) throw new Error('provider auth needs the desktop runtime')
}

/** 결과 값이 글자로 오든 { state } 로 오든 같은 상태 값으로. */
function stateOf(result: unknown) {
  if (typeof result === 'string') return result
  return (result as { state?: string } | null)?.state ?? null
}

export async function getProviderAuth() { need(); return invoke<ProviderAuth[]>('provider_auth_status') }
export async function startProviderLogin(provider: AiProvider) { need(); return stateOf(await invoke('provider_login_start', { provider })) }
export async function cancelProviderLogin(provider: AiProvider) { need(); return stateOf(await invoke('provider_login_cancel', { provider })) }
export async function logoutProvider(provider: AiProvider) { need(); return stateOf(await invoke('provider_logout', { provider })) }
export async function setProviderApiKey(provider: AiProvider, apiKey: string) { need(); return stateOf(await invoke('provider_api_key_set', { provider, apiKey })) }
export async function clearProviderApiKey(provider: AiProvider) { need(); return stateOf(await invoke('provider_api_key_clear', { provider })) }
export async function verifyProviderApiKey(provider: AiProvider) { need(); return stateOf(await invoke('provider_api_key_verify', { provider })) }
/** 첫 실행을 넘어갈 수 있는가(뒤쪽이 정한 불리언 그대로). */
export async function providerFirstRunEligible() { need(); return (await invoke<boolean>('provider_first_run_eligible')) === true }

export async function installProvider(provider: AiProvider) { need(); return stateOf(await invoke('install_external_provider', { provider })) }

export type InstallProgress = {
  provider: AiProvider
  phase: 'installing' | 'verifying' | 'completed' | 'failed'
  receivedBytes: number | null
  totalBytes: number | null
  canCancel: boolean
  reason: string | null
}

export async function listenInstallProgress(onProgress: (value: InstallProgress) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<InstallProgress>('provider-install-progress', (event) => { if (event.payload?.provider) onProgress(event.payload) })
}

/** 그 회사의 도구가 실행되는가. 예전 뒤쪽(runnable 이 없음)은 installed 로 본다. */
function runs(item: ProviderAuth) {
  return typeof item.runnable === 'boolean' ? item.runnable : item.installed
}

/**
 * 저장된 API 키를 연결됨으로 치는가. 계약 한 줄 그대로: runnable && apiKeySaved && keyVerification 이 verified 또는 notSupported.
 * 저장만 됐거나(verifying), 틀렸거나(rejected), 확인하지 못했으면(unavailable) 연결로 치지 않는다.
 */
export function keyConnected(item: ProviderAuth) {
  return runs(item) && item.apiKeySaved && (item.keyVerification === 'verified' || item.keyVerification === 'notSupported')
}

/**
 * 계정이나 API 키로 연결됐는가. 설정·첫 실행의 회사 줄, 입력창의 회사 고르기와 기본 회사, 왼쪽 띠 사용량이 같은 이 조건을 쓴다.
 * 첫 실행의 "시작하기"는 이것이 아니라 providerFirstRunEligible() 을 쓴다(뒤쪽이 정한 값).
 */
export function isConnected(item: ProviderAuth) {
  if (!runs(item)) return false
  return item.state === 'account' || keyConnected(item)
}

/** 저장된 키의 지금 형편(회사 줄에서 쓴다). 계정으로 연결됐거나 키가 없으면 null. */
export function keyProblem(item: ProviderAuth): 'verifying' | 'rejected' | 'unavailable' | null {
  if (item.state === 'account' || !item.apiKeySaved) return null
  if (item.keyVerification === 'verifying') return 'verifying'
  if (item.keyVerification === 'rejected') return 'rejected'
  if (item.keyVerification === 'unavailable') return 'unavailable'
  return null
}

// provider-auth-changed 에 reason 이 붙을 수 있다(2026-11-17): 로그인이 시간 안에 끝나지 않으면 state unavailable + reason timeout.
export async function listenProviderAuth(onChange: (provider: AiProvider, state: string | null, reason: string | null) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ provider?: AiProvider; state?: string; reason?: string | null }>('provider-auth-changed', (event) => {
    if (event.payload?.provider) onChange(event.payload.provider, event.payload.state ?? null, typeof event.payload.reason === 'string' ? event.payload.reason : null)
  })
}
