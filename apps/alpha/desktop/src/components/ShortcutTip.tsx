import { Fragment, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useAvoidPanelBrowser } from '../panel-browser'
import './ShortcutTip.css'

// 올린 뒤 풍선이 뜨기까지(스쳐 지나갈 때 깜빡이지 않게).
const SHOW_DELAY_MS = 350
const EDGE = 8

/**
 * 단축키가 있는 단추의 풍선. 이름은 왼쪽, 단축키는 오른쪽에 키 모양으로(키마다 따로, "+" 는 키 밖에).
 * 브라우저 기본 풍선(title)으로는 키 모양을 낼 수 없어 앱이 그린다. 단추 아래에 뜨고, 창 가장자리에서 잘리지 않게 옮긴다.
 */
export function ShortcutTip({
  label,
  keys = [],
  side = 'bottom',
  children,
}: {
  label: string
  /** 단축키. 없으면 이름만 보인다. */
  keys?: string[]
  /** 풍선이 뜨는 쪽. 아이콘 띠의 단추는 오른쪽. */
  side?: 'bottom' | 'right'
  children: ReactNode
}) {
  const wrap = useRef<HTMLSpanElement>(null)
  const tip = useRef<HTMLSpanElement>(null)
  const timer = useRef<number | null>(null)
  const [open, setOpen] = useState(false)
  const [leaving, setLeaving] = useState(false)
  const [place, setPlace] = useState<{ left: number; top: number } | null>(null)
  // 풍선이 패널 웹뷰와 겹치면 떠 있는 동안 웹뷰를 숨긴다(웹뷰는 HTML 위에 그려진다).
  useAvoidPanelBrowser(tip, open && place !== null)

  const show = () => {
    if (timer.current) window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => {
      setLeaving(false)
      setOpen(true)
    }, SHOW_DELAY_MS)
  }
  const hide = () => {
    if (timer.current) window.clearTimeout(timer.current)
    if (!open) return
    setLeaving(true)
    timer.current = window.setTimeout(() => {
      setOpen(false)
      setLeaving(false)
      setPlace(null)
    }, 110)
  }

  useEffect(() => () => {
    if (timer.current) window.clearTimeout(timer.current)
  }, [])

  // 단추 아래 가운데. 오른쪽·왼쪽 가장자리를 넘으면 안으로, 아래에 자리가 없으면 위로.
  useLayoutEffect(() => {
    if (!open || !wrap.current || !tip.current) return
    const anchor = wrap.current.getBoundingClientRect()
    const box = tip.current.getBoundingClientRect()
    if (side === 'right') {
      const top = Math.max(EDGE, Math.min(anchor.top + anchor.height / 2 - box.height / 2, window.innerHeight - box.height - EDGE))
      setPlace({ left: anchor.right + 8, top })
      return
    }
    let left = anchor.left + anchor.width / 2 - box.width / 2
    left = Math.max(EDGE, Math.min(left, window.innerWidth - box.width - EDGE))
    let top = anchor.bottom + 6
    if (top + box.height > window.innerHeight - EDGE) top = anchor.top - box.height - 6
    setPlace({ left, top })
  }, [open, side])

  return (
    <span
      ref={wrap}
      className="shortcut-tip-anchor"
      onPointerEnter={show}
      onPointerLeave={hide}
      onFocus={show}
      onBlur={hide}
      onPointerDown={hide}
    >
      {children}
      {open && createPortal(
        <span
          ref={tip}
          className="shortcut-tip"
          role="tooltip"
          data-leaving={leaving}
          style={place ? { left: place.left, top: place.top } : { left: -9999, top: -9999 }}
        >
          <span className="shortcut-tip-label">{label}</span>
          {keys.length > 0 && (
            <span className="shortcut-tip-keys">
              {keys.map((key, index) => (
                <Fragment key={`${key}-${index}`}>
                  {index > 0 && <span className="shortcut-tip-plus">+</span>}
                  <kbd>{key}</kbd>
                </Fragment>
              ))}
            </span>
          )}
        </span>,
        document.body,
      )}
    </span>
  )
}
