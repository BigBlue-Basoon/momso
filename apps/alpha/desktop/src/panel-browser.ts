// 오른쪽 패널 브라우저(2026-11-17, 뒤쪽 panel_browser.rs · desktop-panel-browser-contract.md). 사람이 쓰는 패널이다(AI 는 다루지 않는다).
//   명령: panel_browser_open({ url, x, y, width, height }) → 이미 열렸으면 already_open, Google 계정 주소면 google_login_required
//         panel_browser_close(), panel_browser_visibility({ visible }), panel_browser_bounds({ x, y, width, height }) (창 안의 논리 좌표)
//         panel_browser_navigate({ url }), panel_browser_action({ action: back|forward|reload|stop })
//         panel_browser_open_human({ url }): 사람이 쓰는 전용 창(Edge, 없으면 Chrome, 둘 다 없으면 browser_missing)
//         그 밖의 오류: invalid_url, invalid_bounds, invalid_action, unavailable
//   이벤트: panel-browser-page { url, host, secure, loading }, panel-browser-title { title }, panel-browser-history { canGoBack, canGoForward },
//           panel-browser-download { state: started|finished|blocked|failed, name, path, size } (2026-11-18 새 꼴. 다운로드 폴더에 새 파일로 저장, 실행 파일은 막음),
//           panel-browser-google-login { host, siteUrl } (패널 이동을 막고 보냄, siteUrl 은 null 일 수 있음)
// 패널 웹뷰는 몸소 화면 위의 따로 뜨는 층이다. 몸소의 묻는 창·찾기 창 같은 것이 그 자리를 덮는 동안은 웹뷰를 숨긴다(holdPanelBrowser).
//
// 채팅방마다 따로(2026-11-21 뒤쪽 3c76dcf, desktop-browser-chat-isolation-contract.md "작업 중"):
//   panel_browser_select_chat({ chatId }) → { chatId, hasPanel }: 채팅방을 고를 때마다 다른 패널 명령보다 먼저 부르고 끝나기를 기다린다.
//     hasPanel 이 참이면 그 방의 패널(웹뷰)이 이미 있다. 방마다 웹뷰가 따로이고 로그인 쿠키(프로필)만 함께 쓴다.
//   open · open_agent · visibility · bounds · navigate · action · close 는 모두 chatId 를 함께 받고, 고른 방이 아니면 panel_not_active.
//   고른 방이 없을 때 열면 chat_required, 방 번호가 틀리면 invalid_chat.
//   이벤트 panel-browser-page · panel-browser-title · panel-browser-history 에 chatId 가 붙는다(다른 방 것은 그리지 않는다).
// 새 채팅방(2026-11-21 뒤쪽 50cf562, 같은 계약 문서):
//   panel_browser_create_draft() → { chatId, hasPanel }: 방 번호가 없는 화면의 임시 방("draft-…"). 일반 패널 명령에 그 번호를 쓴다.
//   첫 말을 chat_create_with_message 로 저장해 방 번호를 받으면, 그 방을 고르거나 AI 를 부르기 전에
//   panel_browser_adopt_draft({ draftId, chatId }) → { chatId, hasPanel }: 열린 웹뷰는 그대로 두고 주인 방만 바꾼다(뒤쪽은 그 방을 고른 것으로 둔다).
//   임시 방이 고른 방이 아니면 panel_not_active, 그 방에 패널 자리가 이미 있으면 panel_exists(둘 다 덮어쓰지 않음). 방을 못 만들면 임시 패널은 그대로.
//   뒤쪽은 다른 방으로 가도 임시 방의 웹뷰를 숨겨 둘 뿐 닫지 않고, 넘겨준 임시 방을 다시 고르면 draft_adopted. 그래서 화면은 넘겨주기 전까지
//   이번 실행의 임시 방 하나를 다시 고른다(새 채팅방으로 돌아오면 보던 페이지가 그대로). 넘겨준 뒤 새 채팅방으로 가면 새 임시 방을 받는다.
//   panel-browser-popup · panel-browser-download · panel-browser-google-login 에도 chatId 가 붙는다(다운로드 끝은 시작한 방, 넘겨준 뒤면 새 방 번호).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { type RefObject, useEffect, useLayoutEffect, useRef, useSyncExternalStore } from 'react'
import { isDesktopRuntime } from './runtime'

export type PanelBounds = { x: number; y: number; width: number; height: number }
export type PanelPage = { url: string; host: string | null; secure: boolean; loading: boolean }

export const PANEL_BROWSER_ERRORS = ['already_open', 'google_login_required', 'invalid_url', 'invalid_bounds', 'invalid_action', 'browser_missing', 'panel_not_active', 'panel_exists', 'chat_required', 'invalid_chat', 'unavailable'] as const
export type PanelBrowserError = (typeof PANEL_BROWSER_ERRORS)[number]

/** 뒤쪽 오류 글에서 정해 둔 값만 꺼낸다. 모르는 것은 unavailable. */
export function panelBrowserError(error: unknown): PanelBrowserError {
  const text = error instanceof Error ? error.message : String(error)
  return PANEL_BROWSER_ERRORS.find((code) => text.includes(code)) ?? 'unavailable'
}

// ---- 고른 채팅방(위 "채팅방마다 따로") ----
// 패널 명령은 고르기와 함께 한 줄로 세운다: 고르기가 끝나기 전에 그 방의 명령이 나가지 않고, 다른 방을 고른 뒤에는 앞 방의 명령을 보내지 않는다.
// (예전에 여는 요청만 줄을 세운 까닭도 그대로: 둘이 겹치면 뒤쪽이 같은 웹뷰를 두 번 만들려다 뒤의 것이 실패했다. 2026-11-17 실제 앱.)
export type PanelChat = { chatId: string | null; hasPanel: boolean }
let queue: Promise<unknown> = Promise.resolve()
function inLine<T>(job: () => Promise<T>) {
  const run = queue.then(job)
  queue = run.catch(() => {})
  return run
}
/** 화면이 지금 고른 방(null 은 새 채팅방, undefined 는 아직 한 번도 고르지 않음). */
let wanted: string | null | undefined = undefined
/** 뒤쪽이 받은 방. 화면의 패널은 이 방이 자기 방일 때만 움직인다. 새 채팅방이면 임시 방 번호. */
let chosen: PanelChat = { chatId: null, hasPanel: false }
/** 뒤쪽에서 마지막으로 고른 방. */
let backendChat: string | null = null
/** 이번 실행의 새 채팅방 임시 방(panel_browser_create_draft). 첫 말로 생긴 방에 넘겨주면 null. */
let draft: string | null = null
/** 방금 임시 방을 넘겨받은 방. App 이 그 방을 고르기 전 한 번의 그림 동안 새 채팅방 화면의 브라우저가 그 방을 그대로 쓰게. */
let adoptedTo: string | null = null
/** 화면의 방이 뒤쪽에서 쓰는 방 번호(새 채팅방이면 임시 방). */
function roomOf(chatId: string | null | undefined) {
  return chatId ?? draft
}
type Room = { chatId?: unknown; hasPanel?: unknown } | null

/** 채팅방을 고를 때마다(App). 같은 방이면 아무것도 하지 않는다. */
export function selectPanelChat(chatId: string | null) {
  if (adoptedTo !== null) { adoptedTo = null; changed() }
  if (chatId === wanted) return
  wanted = chatId
  if (!isDesktopRuntime()) { chosen = { chatId, hasPanel: false }; changed(); return }
  void inLine(async () => {
    // 그새 다른 방을 골랐으면 이 방은 건너뛴다(보이지 않은 방).
    if (wanted !== chatId) return
    // 이미 뒤쪽이 이 방을 골라 두었으면(빠르게 오가다 같은 방으로 돌아옴) 다시 부르지 않는다.
    const target = roomOf(chatId)
    if (target && backendChat === target && chosen.chatId === target) return
    // 고르는 동안은 어느 방도 고른 방이 아니다: 그 사이 앞 방의 명령이 나가지 않고, 끝나면 패널이 보임·자리를 다시 알린다.
    // (뒤쪽은 고를 때 그 방의 웹뷰를 스스로 보이므로, 숨겨야 하는 방이면 화면이 다시 숨겨야 한다.)
    backendChat = null
    if (chosen.chatId !== null) { chosen = { chatId: null, hasPanel: false }; changed() }
    try {
      let value: Room
      if (chatId === null) {
        // 새 채팅방: 이번 실행의 임시 방을 다시 고른다. 없거나 다시 고르지 못하면 새 임시 방을 받는다.
        value = draft ? await invoke<Room>('panel_browser_select_chat', { chatId: draft }).catch(() => null) : null
        if (!draft || value?.chatId !== draft) {
          value = await invoke<Room>('panel_browser_create_draft')
          if (typeof value?.chatId !== 'string' || !value.chatId) throw new Error('unavailable')
          draft = value.chatId
          changed()
        }
      } else {
        value = await invoke<Room>('panel_browser_select_chat', { chatId })
      }
      const room = roomOf(chatId)
      backendChat = room
      // 답의 방이 맞을 때만 받는다(뒤늦은 답이 다른 방에 그려지지 않게).
      if (wanted === chatId && room && value?.chatId === room) { chosen = { chatId: room, hasPanel: value.hasPanel === true }; changed() }
    } catch {
      // 고르지 못했으면 그 방의 패널은 쓰지 않는다.
      if (wanted === chatId) { chosen = { chatId: null, hasPanel: false }; changed() }
    }
  })
}
export function usePanelChat() {
  return useSyncExternalStore(subscribe, () => chosen)
}
/** 새 채팅방의 임시 방 번호(아직 받지 못했으면 null). 패널의 브라우저 탭이 새 채팅방에서 이 번호를 방 번호로 쓴다. */
export function usePanelDraft() {
  return useSyncExternalStore(subscribe, () => draft ?? adoptedTo)
}
export function currentPanelDraft() {
  return draft
}
const adoptListeners = new Set<(draftId: string, chatId: string) => void>()
/** 임시 방을 첫 말로 생긴 방에 넘겨줬을 때(패널이 브라우저 탭의 방 번호를 바꾼다. 탭은 닫지 않는다). */
export function onPanelDraftAdopted(listener: (draftId: string, chatId: string) => void) {
  adoptListeners.add(listener)
  return () => { adoptListeners.delete(listener) }
}
/**
 * 새 채팅방의 첫 말로 방(chatId)이 생겼을 때, 그 방을 고르거나 AI 를 부르기 전에(App). 임시 방의 웹뷰를 그 방 것으로 넘긴다.
 * 넘기면 뒤쪽은 그 방을 고른 것으로 두므로 화면도 그 방을 고른 것으로 둔다(뒤이은 selectPanelChat(chatId) 는 아무것도 하지 않는다).
 * 임시 방을 받지 못했거나 넘기지 못하면 거짓: 그 방은 보통처럼 고른다(임시 방의 웹뷰는 그대로 남는다).
 */
export function adoptPanelDraft(chatId: string) {
  return inLine(async () => {
    const from = draft
    if (!isDesktopRuntime() || !from || wanted !== null || chosen.chatId !== from) return false
    try {
      const value = await invoke<Room>('panel_browser_adopt_draft', { draftId: from, chatId })
      draft = null
      adoptedTo = chatId
      wanted = chatId
      backendChat = chatId
      chosen = { chatId, hasPanel: value?.hasPanel === true }
      adoptListeners.forEach((listener) => listener(from, chatId))
      changed()
      return true
    } catch {
      return false
    }
  })
}
/** 그 방이 고른 방일 때만 보낸다. 아니면 보내지 않고 panel_not_active 로 끝낸다. */
function command<T>(chatId: string | null, name: string, args: Record<string, unknown> = {}) {
  return inLine(() => {
    if (!chatId || chatId !== roomOf(wanted) || chatId !== chosen.chatId) return Promise.reject(new Error('panel_not_active'))
    return invoke<T>(name, { chatId, ...args })
  })
}

export function openPanelBrowser(chatId: string | null, url: string, bounds: PanelBounds) {
  return command(chatId, 'panel_browser_open', { url, ...bounds }).then((value) => { placed = bounds; return value })
}
export async function closePanelBrowser(chatId: string | null) {
  await command(chatId, 'panel_browser_close')
  placed = null
}
export async function setPanelBrowserVisible(chatId: string | null, visible: boolean) {
  await command(chatId, 'panel_browser_visibility', { visible })
}
export async function setPanelBrowserBounds(chatId: string | null, bounds: PanelBounds) {
  await command(chatId, 'panel_browser_bounds', bounds)
  placed = bounds
}
export async function navigatePanelBrowser(chatId: string | null, url: string) {
  await command(chatId, 'panel_browser_navigate', { url })
}
export async function panelBrowserAction(chatId: string | null, action: 'back' | 'forward' | 'reload' | 'stop') {
  await command(chatId, 'panel_browser_action', { action })
}
/**
 * 바깥 로그인(2026-11-18 여섯째 줄 G75, panel-agent-integration-g52.md): panel_agent_external_login_open({ chatId }) → { state: 'login' }.
 * 몸소가 원래 허용된 사이트를 그 채팅방의 몸소 전용 브라우저 프로필로 연다(구글 인증 주소를 옮기지 않음). 패널과 쿠키를 나누지 않는다.
 */
export async function openExternalLogin(chatId: string) {
  await invoke('panel_agent_external_login_open', { chatId })
}
export async function openHumanBrowser(url: string) {
  await invoke('panel_browser_open_human', { url })
}
// ---- AI 가 패널을 쓸 때(2026-11-18 계약 panel-agent-integration-g52.md, 뒤쪽 feat/desktop-panel-agent) ----
//   이벤트 panel-agent-open-request { chatId }: 패널이 없으면 뒤쪽이 보내고 20초 기다린다. 화면은 브라우저 탭을 열고 실제 자리로
//     panel_browser_open_agent({ x, y, width, height }) 를 불러 빈 패널을 연다(주소는 AI 가 승인 뒤 연다).
//   panel_agent_status({ chatId }) → { ownerChatId, humanControls }
//   panel_agent_take_over({ chatId }) / panel_agent_resume({ chatId }) → 이벤트 panel-agent-control-changed { chatId, state: human | available }.
//     AI 조작 시작·끝도 같은 이벤트의 started / ended. 오류: panel_busy(다른 채팅방이 씀), human_takeover, panel_closed, panel_page_changed.
//   계약에 없는 것: 로그인 화면이라 사람에게 넘겼다는 이벤트·값(화면은 아직 그 안내를 그리지 않는다).
export async function openPanelAgent(chatId: string | null, bounds: PanelBounds) {
  await command(chatId, 'panel_browser_open_agent', bounds)
  placed = bounds
}
export async function panelAgentStatus(chatId: string) {
  const value = await invoke<{ ownerChatId?: unknown; humanControls?: unknown } | null>('panel_agent_status', { chatId })
  return { ownerChatId: typeof value?.ownerChatId === 'string' ? value.ownerChatId : null, humanControls: value?.humanControls === true }
}
export async function takeOverPanel(chatId: string) {
  await invoke('panel_agent_take_over', { chatId })
}
export async function resumePanelAgent(chatId: string) {
  await invoke('panel_agent_resume', { chatId })
}
/**
 * 2026-11-18 뒤쪽 cdd3ebe(G57b): panel-agent-control-changed { chatId, state, reason, provider, chatTitle }.
 *   state: started | ended | human | available | login. login 의 reason: login_page | one_time_code | captcha | verification |
 *   google_login_external(구글 로그인은 패널에서 못 해 바깥 창에서). 로그인이 끝나면 panel_agent_resume → 뒤쪽이 확인하면 started,
 *   아직이면 human_required 오류. ended 의 reason: panel_not_opened(열어 달라고 했는데 AI 의 턴이 끝남) 또는 null.
 *   provider: codex | claude | gemini, chatTitle: 그 채팅방 이름(없으면 null).
 */
export type PanelAgentControl = {
  chatId: string
  state: 'started' | 'ended' | 'human' | 'available' | 'login' | string
  reason: string | null
  provider: string | null
  chatTitle: string | null
  /** google_login_external 일 때(2026-11-18 뒤쪽 2ae5d7e G58b): 열 로그인 주소(accounts.google.com)와 지금 패널의 사이트. */
  loginUrl?: string | null
  siteUrl?: string | null
}
// browser-human-required { chatId, reason }(login | password | one_time_code | captcha | verification | external_login): 몸소 브라우저가 사람에게 넘길 때.
// 패널 AI 의 채팅방이면 패널 위 로그인 줄로도 보인다(뒤쪽은 보통 panel-agent-control-changed login 도 함께 보내지만, 그것이 없을 때를 위해).
export async function listenPanelAgent(handlers: { onOpenRequest?: (chatId: string | null) => void; onControl?: (control: PanelAgentControl) => void; onHumanRequired?: (chatId: string, reason: string, loginUrl: string | null) => void
  /**
   * 로그인 뒤 [AI에게 돌려주기]로 몸소가 그 채팅방에 "로그인을 마쳤습니다. 이어서 해 주세요."를 보내 새 턴이 시작됨
   * (2026-11-18 여섯째 줄 G68): panel-agent-resume-started { chatId, userMessageId, text, provider, skill, originalMessageId }.
   */
  onResumeStarted?: (event: { chatId: string; userMessageId: string | null; text: string | null; provider: string | null }) => void
  /** panel-agent-external-login-opened { chatId, state: 'login', message }: 몸소 브라우저 창이 열림. 뒤쪽 message 는 화면에 쓰지 않는다. */
  onExternalOpened?: (chatId: string) => void
  /** panel-agent-external-continued { chatId, state: 'started', message }: 로그인 뒤 AI 가 그 바깥 창에서 이어 함. */
  onExternalContinued?: (chatId: string, state: string) => void }) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<{ chatId?: unknown }>('panel-agent-open-request', (event) => handlers.onOpenRequest?.(typeof event.payload?.chatId === 'string' ? event.payload.chatId : null)),
    listen<{ chatId?: unknown; state?: unknown; reason?: unknown; provider?: unknown; chatTitle?: unknown; loginUrl?: unknown; siteUrl?: unknown }>('panel-agent-control-changed', (event) => {
      const p = event.payload ?? {}
      const text = (value: unknown) => (typeof value === 'string' && value.trim() ? value : null)
      if (typeof p.chatId === 'string' && typeof p.state === 'string') {
        handlers.onControl?.({ chatId: p.chatId, state: p.state, reason: text(p.reason), provider: text(p.provider), chatTitle: text(p.chatTitle), loginUrl: text(p.loginUrl), siteUrl: text(p.siteUrl) })
      }
    }),
    listen<{ chatId?: unknown }>('panel-agent-external-login-opened', (event) => {
      if (typeof event.payload?.chatId === 'string') handlers.onExternalOpened?.(event.payload.chatId)
    }),
    listen<{ chatId?: unknown; state?: unknown }>('panel-agent-external-continued', (event) => {
      const p = event.payload ?? {}
      if (typeof p.chatId === 'string') handlers.onExternalContinued?.(p.chatId, typeof p.state === 'string' ? p.state : 'started')
    }),
    listen<{ chatId?: unknown; userMessageId?: unknown; text?: unknown; provider?: unknown }>('panel-agent-resume-started', (event) => {
      const p = event.payload ?? {}
      if (typeof p.chatId !== 'string') return
      const text = (value: unknown) => (typeof value === 'string' && value ? value : null)
      handlers.onResumeStarted?.({ chatId: p.chatId, userMessageId: text(p.userMessageId), text: text(p.text), provider: text(p.provider) })
    }),
    listen<{ chatId?: unknown; reason?: unknown; loginUrl?: unknown }>('browser-human-required', (event) => {
      const p = event.payload ?? {}
      if (typeof p.chatId === 'string' && typeof p.reason === 'string') handlers.onHumanRequired?.(p.chatId, p.reason, typeof p.loginUrl === 'string' && p.loginUrl ? p.loginUrl : null)
    }),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 이번 실행에서 다 받은 파일을 첨부할 선택으로 기록한다(그 뒤 첨부가 그 경로를 받아들인다). */
export async function selectDownloadForAttach(path: string) {
  await invoke('panel_browser_download_select', { path })
}
/** 이번 실행에서 다 받은 파일을 탐색기에서 골라 보인다(panel_download_reveal, 그 밖의 경로는 selection_required). */
export async function revealDownload(path: string) {
  await invoke('panel_download_reveal', { path })
}

/**
 * 팝업(2026-11-18 뒤쪽 144bbd7): panel-browser-popup { state: opened | closed, host, openerHost, title, reason, chatId }.
 * 닫힘의 reason: user | site | panel_closed | unknown. 팝업마다 이름이 없어 여럿이면 마지막 것만 보인다. 둘째 값은 그 방(옛 뒤쪽이면 null).
 */
export type PanelPopup = { state: string; host: string | null; openerHost: string | null; title: string | null; reason: string | null }
export async function listenPanelPopup(onPopup: (popup: PanelPopup, chatId: string | null) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<Partial<Record<keyof PanelPopup | 'chatId', unknown>>>('panel-browser-popup', (event) => {
    const p = event.payload ?? {}
    const text = (v: unknown) => (typeof v === 'string' && v ? v : null)
    if (typeof p.state === 'string') onPopup({ state: p.state, host: text(p.host), openerHost: text(p.openerHost), title: text(p.title), reason: text(p.reason) }, text(p.chatId))
  })
}

export type PanelDownload = { state: 'started' | 'finished' | 'blocked' | 'failed' | string; name: string | null; path: string | null; size: number | null }

/** 둘째 값은 그 이벤트의 채팅방(chatId, 없으면 null). 다운로드·Google 로그인에도 방이 붙는다(2026-11-21 뒤쪽 50cf562). */
export async function listenPanelBrowser(handlers: {
  onPage?: (page: PanelPage, chatId: string | null) => void
  onTitle?: (title: string, chatId: string | null) => void
  onHistory?: (history: { canGoBack: boolean | null; canGoForward: boolean | null }, chatId: string | null) => void
  onDownload?: (download: PanelDownload, chatId: string | null) => void
  onGoogleLogin?: (siteUrl: string | null, chatId: string | null) => void
}) {
  if (!isDesktopRuntime()) return () => {}
  const room = (value: unknown) => (typeof value === 'string' && value ? value : null)
  const stops = await Promise.all([
    listen<Partial<PanelPage> & { chatId?: unknown }>('panel-browser-page', (event) => {
      const p = event.payload ?? {}
      if (typeof p.url === 'string') handlers.onPage?.({ url: p.url, host: typeof p.host === 'string' && p.host ? p.host : null, secure: p.secure === true, loading: p.loading === true }, room(p.chatId))
    }),
    listen<{ title?: unknown; chatId?: unknown }>('panel-browser-title', (event) => { if (typeof event.payload?.title === 'string') handlers.onTitle?.(event.payload.title, room(event.payload.chatId)) }),
    listen<{ canGoBack?: unknown; canGoForward?: unknown; chatId?: unknown }>('panel-browser-history', (event) => handlers.onHistory?.({
      canGoBack: typeof event.payload?.canGoBack === 'boolean' ? event.payload.canGoBack : null,
      canGoForward: typeof event.payload?.canGoForward === 'boolean' ? event.payload.canGoForward : null,
    }, room(event.payload?.chatId))),
    listen<{ state?: unknown; name?: unknown; path?: unknown; size?: unknown; chatId?: unknown }>('panel-browser-download', (event) => {
      const p = event.payload ?? {}
      if (typeof p.state !== 'string') return
      handlers.onDownload?.({
        state: p.state,
        name: typeof p.name === 'string' && p.name ? p.name : null,
        path: typeof p.path === 'string' && p.path ? p.path : null,
        size: typeof p.size === 'number' && Number.isFinite(p.size) ? p.size : null,
      }, room(p.chatId))
    }),
    listen<{ siteUrl?: unknown; chatId?: unknown }>('panel-browser-google-login', (event) => handlers.onGoogleLogin?.(typeof event.payload?.siteUrl === 'string' && event.payload.siteUrl ? event.payload.siteUrl : null, room(event.payload?.chatId))),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 사람이 친 주소. 앞에 http(s):// 가 없으면 https:// 를 붙인다(주소만 받는다. 찾기 말은 뒤쪽이 invalid_url 로 거절). */
export function typedAddress(text: string) {
  const value = text.trim()
  if (!value) return null
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `https://${value}`
}

// ---- 가림: 몸소 것이 웹뷰 자리를 덮는 동안 웹뷰를 숨긴다. 잡은 수가 0 이 되면 다시 보인다. ----
let holds = 0
let shown = false
const listeners = new Set<() => void>()
function changed() { listeners.forEach((listener) => listener()) }
function subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener) } }

// 웹뷰가 지금 놓인 자리(마지막으로 뒤쪽에 알린 값). 메뉴·풍선이 그 자리와 겹치는지 볼 때 쓴다.
let placed: PanelBounds | null = null

/**
 * 메뉴·풍선·작은 창처럼 HTML 위에 잠깐 뜨는 것이 웹뷰 자리와 겹치면, 떠 있는 동안 웹뷰를 숨긴다(2026-11-18 실제 앱 B71).
 * 웹뷰는 따로 뜨는 층이라 HTML 이 그 위에 그려지지 않는다. 겹치지 않으면 아무것도 하지 않는다. 열린 동안 한 번 숨기면 닫힐 때까지 둔다(깜빡임 없게).
 */
export function useAvoidPanelBrowser(element: RefObject<HTMLElement | null>, open: boolean) {
  const release = useRef<(() => void) | null>(null)
  useLayoutEffect(() => {
    if (!open) {
      release.current?.()
      release.current = null
      return
    }
    if (release.current || !shown || !placed) return
    const rect = element.current?.getBoundingClientRect()
    if (!rect || rect.width < 1 || rect.height < 1) return
    const over = rect.right > placed.x && rect.left < placed.x + placed.width && rect.bottom > placed.y && rect.top < placed.y + placed.height
    if (over) release.current = holdPanelBrowser()
  })
  useEffect(() => () => { release.current?.(); release.current = null }, [])
}

/** 덮는 것이 열리면 잡고, 닫히면 놓는다. 놓는 함수를 돌려준다. */
export function holdPanelBrowser() {
  holds += 1
  changed()
  let released = false
  return () => { if (released) return; released = true; holds -= 1; changed() }
}
/** 열려 있는 동안 웹뷰를 숨기는 부품(묻는 창·찾기 창·크게 보기)이 부른다. */
export function usePanelBrowserHold(active = true) {
  useEffect(() => (active ? holdPanelBrowser() : undefined), [active])
}
export function usePanelBrowserHeld() {
  return useSyncExternalStore(subscribe, () => holds > 0)
}

/** 패널 브라우저 탭이 지금 화면에 보이는가(알림 줄 자리를 옮기는 데 쓴다). */
export function setPanelBrowserShown(value: boolean) {
  if (shown === value) return
  shown = value
  changed()
}
export function usePanelBrowserShown() {
  return useSyncExternalStore(subscribe, () => shown)
}

/** 몸소 안의 웹 링크를 패널에서 열어 달라고 알린다(App 이 패널을 열고, 패널이 브라우저 탭을 연다). */
export function requestPanelBrowser(url: string | null) {
  window.dispatchEvent(new CustomEvent('momso:open-web', { detail: { url } }))
}
