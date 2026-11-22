// 몸소 전용 브라우저(바깥 사이트 스킬). 계약: 뒤쪽 docs/development/desktop-screen-backend-contract.md "브라우저", browser-bridge-screen-contract.md.
//   이벤트 browser-human-required { chatId, reason }: reason = login | password | one_time_code | captcha | verification.
//     선생님이 몸소가 연 브라우저 창에서 직접 하고 채팅으로 알려 주면 같은 채팅방의 브라우저를 이어 쓴다.
//   이벤트 browser-session-updated { chatId, open }: 몸소가 브라우저를 열거나 닫을 때. browser_session_status({ chatId }) → { chatId, open }
//     (사람이 창을 직접 닫았으면 이 명령으로 다시 확인한다).
//   승인은 기존 agent-approval-request: kind 'browser_action_scope'(읽기만에서 바꾸는 일로) / 'browser_action'(누르기·입력 한 번마다).
//     targets[0].path = 사이트 출처, reason = 'scope' 또는 효과(publish, delete, notify, refund, payment, irreversible, unknown). options allow_once·deny·cancel.
//     reason 'site_access'(2026-11-21 뒤쪽 3c76dcf): 이 채팅방에서 그 호스트의 누르기·입력에 대한 결정. 허락·거절 모두 앱이 켜져 있는 동안 같은 방·호스트에 기억된다.
//   자동 심사 단계에서도 브라우저는 위 site_access 승인으로 묻는다(2026-11-21 뒤쪽 50cf562, desktop-browser-chat-isolation-contract.md).
//     3c76dcf 의 browser-auto-review-unavailable 이벤트는 50cf562 에서 없어졌다(화면도 듣지 않는다).
import { invoke, listen } from './host'
import { hasAppBackend, isDesktopRuntime } from './runtime'

export type BrowserHumanReason = 'login' | 'password' | 'one_time_code' | 'captcha' | 'verification' | string

export async function browserSessionStatus(chatId: string) {
  if (!isDesktopRuntime()) return false
  const value = await invoke<{ open?: boolean } | null>('browser_session_status', { chatId })
  return Boolean(value?.open)
}

export async function listenBrowser(handlers: {
  onHumanRequired: (chatId: string, reason: BrowserHumanReason) => void
  onSession: (chatId: string, open: boolean) => void
}) {
  // 휴대폰 몸소도 browser-human-required 를 받는다(server-client.ts, S23). browser-session-updated 는 데스크톱만 온다.
  if (!hasAppBackend()) return () => {}
  const stops = await Promise.all([
    listen<{ chatId: string; reason: string }>('browser-human-required', (event) => handlers.onHumanRequired(event.payload.chatId, event.payload.reason)),
    listen<{ chatId: string; open: boolean }>('browser-session-updated', (event) => handlers.onSession(event.payload.chatId, Boolean(event.payload.open))),
  ])
  return () => stops.forEach((stop) => stop())
}
