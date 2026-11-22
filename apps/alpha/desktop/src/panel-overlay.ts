// 패널 위에 띄우는 앱 창(2026-11-21 줄1 S13, 계약: 공유 Git hq/s13-done 의 docs/development/desktop-panel-overlay-contract.md).
//   메인 웹뷰: panel_overlay_open({ input: { kind, chatId, x, y, width, height } }) → { id }. 좌표·크기는 메인 창 콘텐츠 기준 논리 픽셀이고,
//     그림자가 잘리지 않게 그림자를 포함한 바깥 크기를 보낸다. chatId 는 지금 고른 패널의 방이어야 한다(아니면 panel_not_active).
//   떠 있는 창: 웹뷰 이름 panel-overlay, 메인과 같은 index.html. panel_overlay_state() → { id, kind, chatId, x, y, width, height } 또는 null 을 읽어 그 내용만 그린다.
//   닫기: panel_overlay_close({ id }) (오래된 id 는 새 창을 닫지 않음). 메인에 panel-overlay-closed { id, reason } —
//     closed(명시) · outside(초점이 밖으로) · chat_changed · panel_hidden · window_changed(이동·크기·배율·숨김) · replaced · failed.
//   동시에 하나. 창 html·body 는 투명, 둥근 모서리·그림자는 화면 CSS.
// 계약의 상태에는 kind 밖에 내용을 실을 칸이 없어, 떠 있는 창이 그릴 것(사이트·페이지 주소·패널 자리)을 kind 에 "종류:JSON" 으로 싣는다(비밀번호는 싣지 않음).
// 떠 있는 창에서 메인으로 알릴 것(열고 채우기가 채움)은 Tauri 이벤트로 보낸다(둘 다 같은 앱 권한).
import { emit } from '@tauri-apps/api/event'
import { invoke, listen } from './host'

export const OVERLAY_WEBVIEW = 'panel-overlay'

/**
 * 저장된 로그인 카드의 그림자 자리(떠 있는 창 = 카드 + 이 여백). 카드 그림자 0 12px 32px · 0 3px 10px 가 닿는 곳:
 * 위 32−12=20, 옆 32, 아래 12+32=44. 창의 이 여백은 투명이고, 누르면 바깥 누름으로 닫는다.
 */
export const LOGIN_CARD_MARGIN = { top: 20, side: 32, bottom: 44 }

export type OverlayRect = { x: number; y: number; width: number; height: number }
export type OverlayState = OverlayRect & { id: string; kind: string; chatId: string }
export type OverlayClosedReason = 'closed' | 'outside' | 'chat_changed' | 'panel_hidden' | 'window_changed' | 'replaced' | 'failed'

/** 이 웹뷰가 떠 있는 창인가(웹뷰 이름으로 가른다). */
export function isOverlayWebview() {
  try {
    const internals = (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWebview?: { label?: unknown } } } }).__TAURI_INTERNALS__
    return internals?.metadata?.currentWebview?.label === OVERLAY_WEBVIEW
  } catch {
    return false
  }
}

/** 떠 있는 창의 종류와 실을 것을 kind 한 글로. */
export function overlayKind(type: string, payload: unknown) {
  return `${type}:${JSON.stringify(payload)}`
}

export function readOverlayKind(kind: string): { type: string; payload: unknown } {
  const at = kind.indexOf(':')
  if (at < 0) return { type: kind, payload: null }
  try {
    return { type: kind.slice(0, at), payload: JSON.parse(kind.slice(at + 1)) }
  } catch {
    return { type: kind.slice(0, at), payload: null }
  }
}

export async function openPanelOverlay(kind: string, chatId: string, rect: OverlayRect) {
  const value = await invoke<{ id?: unknown }>('panel_overlay_open', { input: { kind, chatId, ...rect } })
  if (typeof value?.id !== 'string') throw new Error('unavailable')
  return value.id
}

export function closePanelOverlay(id: string) {
  return invoke<void>('panel_overlay_close', { id })
}

export async function readPanelOverlay(): Promise<OverlayState | null> {
  const value = await invoke<Partial<OverlayState> | null>('panel_overlay_state')
  if (!value || typeof value.id !== 'string' || typeof value.kind !== 'string' || typeof value.chatId !== 'string') return null
  const numbers = [value.x, value.y, value.width, value.height]
  if (!numbers.every((n) => typeof n === 'number' && Number.isFinite(n))) return null
  return value as OverlayState
}

export function listenPanelOverlayClosed(onClosed: (id: string, reason: OverlayClosedReason | string) => void) {
  return listen<{ id?: unknown; reason?: unknown }>('panel-overlay-closed', (event) => {
    const p = event.payload ?? {}
    if (typeof p.id === 'string') onClosed(p.id, typeof p.reason === 'string' ? p.reason : 'closed')
  })
}

// ---- 떠 있는 창 → 메인: 저장된 로그인 열고 채우기가 채움(메인이 패널을 보이게 두고 알린다) ----
const FILLED_EVENT = 'momso-overlay-saved-login-filled'

export function notifyOverlayFilled(chatId: string) {
  return emit(FILLED_EVENT, { chatId })
}

export function listenOverlayFilled(onFilled: (chatId: string) => void) {
  return listen<{ chatId?: unknown }>(FILLED_EVENT, (event) => {
    if (typeof event.payload?.chatId === 'string') onFilled(event.payload.chatId)
  })
}
