import { useEffect, useRef, useState } from 'react'
import { closePanelOverlay, LOGIN_CARD_MARGIN, notifyOverlayFilled, readOverlayKind, readPanelOverlay, type OverlayRect, type OverlayState } from '../panel-overlay'
import { notifyPinsChanged, requestExtensionsPage } from '../panel-extensions'
import { openAndFillSavedLogin } from '../saved-logins'
import { ExtensionsMenu } from './ExtensionsMenu'
import { W } from '../words'
import { SavedLoginsCard } from './SavedLoginsCard'

type SavedLoginsPayload = { site: string | null; pageUrl: string | null; spot: OverlayRect | null }

function readSavedLoginsPayload(value: unknown): SavedLoginsPayload {
  const p = (value && typeof value === 'object' ? value : {}) as Record<string, unknown>
  const spot = p.spot && typeof p.spot === 'object' ? (p.spot as Record<string, unknown>) : null
  const rect = spot && ['x', 'y', 'width', 'height'].every((key) => typeof spot[key] === 'number' && Number.isFinite(spot[key])) ? (spot as unknown as OverlayRect) : null
  return { site: typeof p.site === 'string' ? p.site : null, pageUrl: typeof p.pageUrl === 'string' ? p.pageUrl : null, spot: rect }
}

/**
 * 떠 있는 창(웹뷰 panel-overlay)의 화면(2026-11-21 S13). 메인 화면 전체는 그리지 않고, panel_overlay_state 를 읽어 그 종류의 내용만 그린다.
 * 창 배경은 투명, 내용 둘레의 그림자 자리는 투명한 여백(누르면 바깥 누름으로 닫음). Escape 는 panel_overlay_close.
 *   saved-logins: 저장된 로그인 카드. 열고 채우기가 채우면 메인에 알리고(메인이 패널을 보이게 두고 한 줄 알림) 창을 닫는다.
 *   extensions: 퍼즐 단추의 메뉴(ExtensionsMenu). 관리·가져오기는 메인에 그 채팅방 패널의 관리 화면을 열라고 알리고 창을 닫는다. 고정을 바꾸면 메인의 고정 단추를 다시 그리게 알린다.
 */
export function PanelOverlayRoot() {
  const [state, setState] = useState<OverlayState | null | 'loading'>('loading')
  const card = useRef<HTMLElement>(null)

  useEffect(() => {
    document.documentElement.classList.add('is-panel-overlay')
    let alive = true
    readPanelOverlay().then((value) => { if (alive) setState(value) }).catch(() => { if (alive) setState(null) })
    return () => { alive = false }
  }, [])
  useEffect(() => {
    if (state && state !== 'loading') requestAnimationFrame(() => card.current?.focus())
  }, [state])

  if (state === 'loading' || state === null) return null
  const close = () => { void closePanelOverlay(state.id).catch(() => {}) }
  const { type, payload } = readOverlayKind(state.kind)
  const m = LOGIN_CARD_MARGIN
  if (type === 'extensions') {
    return (
      <div
        className="panel-overlay-root"
        onPointerDown={(event) => { if (event.target === event.currentTarget) close() }}
        onKeyDown={(event) => {
          if (event.key !== 'Escape' || (event.target as HTMLElement).closest('.chat-menu')) return
          event.preventDefault()
          close()
        }}
      >
        <section
          ref={card}
          className="ext-menu-card"
          role="dialog"
          aria-label={W.extensions.button}
          tabIndex={-1}
          style={{ top: m.top, left: m.side, width: state.width - m.side * 2, maxHeight: state.height - m.top - m.bottom }}
        >
          <ExtensionsMenu
            onManage={(section) => { void requestExtensionsPage(state.chatId, section).catch(() => {}).finally(close) }}
            onPinsChanged={() => { void notifyPinsChanged().catch(() => {}) }}
          />
        </section>
      </div>
    )
  }
  if (type !== 'saved-logins') return null
  const context = readSavedLoginsPayload(payload)

  return (
    <div
      className="panel-overlay-root"
      onPointerDown={(event) => { if (event.target === event.currentTarget) close() }}
      onKeyDown={(event) => {
        // 카드 안의 작은 메뉴에서 누른 Escape 는 그 메뉴만 닫는다(메뉴가 스스로 닫음).
        if (event.key !== 'Escape' || (event.target as HTMLElement).closest('.chat-menu')) return
        event.preventDefault()
        close()
      }}
    >
      <section
        ref={card}
        className="panel-login-card is-overlay"
        role="dialog"
        aria-label={W.savedLogins.title}
        tabIndex={-1}
        style={{ top: m.top, left: m.side, width: state.width - m.side * 2, height: state.height - m.top - m.bottom }}
      >
        <SavedLoginsCard
          site={context.site}
          pageUrl={context.pageUrl}
          onOpenAndFill={context.spot ? async (login) => {
            const status = await openAndFillSavedLogin(state.chatId, login.id, context.spot as OverlayRect)
            if (status === 'filled') {
              await notifyOverlayFilled(state.chatId).catch(() => {})
              close()
            }
            return status
          } : undefined}
        />
      </section>
    </div>
  )
}
