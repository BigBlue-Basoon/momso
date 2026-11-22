// AI·GitHub 로그인을 몸소 안에서(2026-11-18, 넷째 줄 · desktop-cli-login-contract.md, 화면 폴더 뒤쪽 6b21644 provider_auth.rs).
//   provider-login-device { provider: 'github' | 'codex', url, code, expiresInSeconds | null }: 장치 코드 로그인. 화면은 코드를 크게, 복사 단추.
//     코드와 주소는 채팅·기록·저장소에 남기지 않는다(화면 메모리에만).
//   provider-login-device-closed { provider }: 그 코드를 지운다.
//   provider-login-browser { provider: 'claude' | 'gemini', mode: 'default_browser' }: 기본 브라우저로 로그인한다는 안내(제미나이 oauth-personal 도 같은 이벤트).
//     [확인]은 provider_login_browser_ack({ provider }) — 뒤쪽은 그 뒤(또는 30초 뒤)에 로그인 창을 연다. provider-login-browser-closed { provider } 에 거둔다(2026-11-18 뒤쪽 6b21644).
//   로그인 페이지는 뒤쪽이 panel-agent-open-request 로 오른쪽 패널에 열어 달라고 하고 20초 기다린 뒤, 안 되면 몸소 브라우저 창으로 연다.
//   취소: provider_login_cancel({ provider }).
import { invoke, listen } from './host'
import { isDesktopRuntime, isNetworkRuntime } from './runtime'

export type DeviceLogin =
  | { kind: 'device'; provider: string; url: string; code: string; expiresAt: number | null }
  | { kind: 'browser'; provider: string }

type DeviceLoginHandlers = { onShow: (login: DeviceLogin) => void; onClose: (provider: string) => void }

/** 데스크톱 App 의 로그인 코드 카드. */
export async function listenDeviceLogin(handlers: DeviceLoginHandlers) {
  if (!isDesktopRuntime()) return () => {}
  return subscribeDeviceLogin(handlers)
}

/** momso anywhere(네트워크 길)의 AI 연결 화면(같은 이벤트를 server-client.ts 가 보낸다). */
export async function listenDeviceLoginAnywhere(handlers: DeviceLoginHandlers) {
  if (!isNetworkRuntime()) return () => {}
  return subscribeDeviceLogin(handlers)
}

async function subscribeDeviceLogin(handlers: DeviceLoginHandlers) {
  const stops = await Promise.all([
    listen<{ provider?: unknown; url?: unknown; code?: unknown; expiresInSeconds?: unknown }>('provider-login-device', (event) => {
      const p = event.payload ?? {}
      if (typeof p.provider !== 'string' || typeof p.url !== 'string' || typeof p.code !== 'string' || !p.code.trim()) return
      const seconds = typeof p.expiresInSeconds === 'number' && p.expiresInSeconds > 0 ? p.expiresInSeconds : null
      handlers.onShow({ kind: 'device', provider: p.provider, url: p.url, code: p.code.trim(), expiresAt: seconds ? Date.now() + seconds * 1000 : null })
    }),
    listen<{ provider?: unknown }>('provider-login-device-closed', (event) => {
      if (typeof event.payload?.provider === 'string') handlers.onClose(event.payload.provider)
    }),
    listen<{ provider?: unknown; mode?: unknown }>('provider-login-browser', (event) => {
      if (typeof event.payload?.provider === 'string') handlers.onShow({ kind: 'browser', provider: event.payload.provider })
    }),
    listen<{ provider?: unknown }>('provider-login-browser-closed', (event) => {
      if (typeof event.payload?.provider === 'string') handlers.onClose(event.payload.provider)
    }),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 기본 브라우저 안내를 읽었다고 알린다(뒤쪽이 로그인 창을 바로 연다). */
export async function acknowledgeBrowserLogin(provider: string) {
  await invoke('provider_login_browser_ack', { provider })
}

/**
 * GitHub 로그인 정보가 안전하지 않은 곳에 남았을 때(2026-11-18 넷째 줄 D33·D34):
 * provider-login-insecure-storage { provider: 'github', action: removed | cleanup_failed | verification_required }.
 */
export async function listenLoginStorage(onEvent: (provider: string, action: string) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ provider?: unknown; action?: unknown }>('provider-login-insecure-storage', (event) => {
    const p = event.payload ?? {}
    if (typeof p.provider === 'string' && typeof p.action === 'string') onEvent(p.provider, p.action)
  })
}

export async function cancelLogin(provider: string) {
  await invoke('provider_login_cancel', { provider })
}
