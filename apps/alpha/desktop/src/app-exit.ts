// 트레이 "종료"(2026-11-20 뒤쪽 6553614, tray_lifecycle.rs). 창의 X 는 숨기기만 하고(뒤쪽), 진짜 종료는 트레이에서만 한다.
//   이벤트 app-exit-requested { requestId }: 뒤쪽이 창을 다시 보이고 보낸다. 화면이 고치던 글을 처리한 뒤
//   명령 app_exit_respond({ requestId, decision: 'exit' | 'stay' }) → { state: 'accepted' | 'stale' | 'invalid' | 'unavailable' }.
//   이벤트 app-exit-request-closed { requestId, reason: 'approved' | 'stayed' | 'timeout' }: 5분 동안 답이 없으면 timeout(종료 취소, 늦은 답은 stale).
// 듣기는 앱에 하나(App 이 켠다). 고치던 글을 아는 곳(오른쪽 패널)이 떠 있으면 그쪽이 묻고 답한다. 없으면 고치던 글도 없으니 바로 exit.
import { invoke, listen } from './host'
import { isDesktopRuntime } from './runtime'

export type ExitDecision = 'exit' | 'stay'
export type ExitHandler = {
  /** 종료 요청이 왔다. 고치던 글을 묻고 끝나면 respondAppExit 로 답한다. */
  ask: (requestId: string) => void
  /** 뒤쪽이 요청을 닫았다(timeout 이면 떠 있던 물음을 거둔다). */
  closed: (requestId: string, reason: string | null) => void
}

let handler: ExitHandler | null = null

/** 고치던 글을 아는 곳이 묻는 쪽으로 선다. 돌려준 함수로 내려온다. */
export function setExitHandler(next: ExitHandler) {
  handler = next
  return () => { if (handler === next) handler = null }
}

export async function respondAppExit(requestId: string, decision: ExitDecision) {
  if (!isDesktopRuntime()) return null
  const value = await invoke<{ state?: string } | null>('app_exit_respond', { requestId, decision }).catch(() => null)
  return value?.state ?? null
}

/** 앱에 하나만 켠다. 돌려준 함수로 끈다. */
export async function listenAppExit() {
  if (!isDesktopRuntime()) return () => {}
  const stopRequest = await listen<{ requestId?: string }>('app-exit-requested', (event) => {
    const id = event.payload?.requestId
    if (!id) return
    if (handler) handler.ask(id)
    else void respondAppExit(id, 'exit')
  })
  const stopClosed = await listen<{ requestId?: string; reason?: string | null }>('app-exit-request-closed', (event) => {
    const id = event.payload?.requestId
    if (id) handler?.closed(id, event.payload?.reason ?? null)
  })
  return () => { stopRequest(); stopClosed() }
}
