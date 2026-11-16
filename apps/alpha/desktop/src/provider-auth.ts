// 설정의 AI 로그인. 계약: momso-desktop-backend/outputs/2026-11-16-momso-provider-login-contract.md
//   provider_auth_status() → [{ provider, installed, state: signedOut|account|apiKey|unavailable, accountLabel, planLabel, apiKeySaved, methods: ('account'|'apiKey')[], active }]
//   provider_login_start({ provider }) → 'inProgress' | 'unavailable'
//   provider_login_cancel({ provider }) → 'cancelled' | 'notInProgress' | 'unavailable'
//   provider_logout({ provider }) → 'signedOut' | 'inProgress' | 'unavailable'
//   provider_api_key_set({ provider, apiKey }) → 'keySaved' | 'invalidKey' | 'unavailable'
//   provider_api_key_clear({ provider }) → 'keyCleared' | 'unavailable'
//   이벤트 provider-auth-changed { provider, state } (받으면 상태를 다시 읽는다)
// API 키는 받아서 바로 넘기기만 한다. 화면은 키를 어디에도 남기지 않는다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AiProvider } from './ai-models'
import { isDesktopRuntime } from './runtime'

export type ProviderAuth = {
  provider: AiProvider
  installed: boolean
  state: 'signedOut' | 'account' | 'apiKey' | 'unavailable'
  accountLabel: string | null
  planLabel: string | null
  apiKeySaved: boolean
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

export async function listenProviderAuth(onChange: (provider: AiProvider, state: string | null) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ provider?: AiProvider; state?: string }>('provider-auth-changed', (event) => {
    if (event.payload?.provider) onChange(event.payload.provider, event.payload.state ?? null)
  })
}
