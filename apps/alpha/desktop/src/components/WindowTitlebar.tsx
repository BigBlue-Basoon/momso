import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ArrowLeft, ArrowRight, Copy, Minus, PanelLeft, Search, Square, X } from 'lucide-react'
import { prepareWindowShift } from '../window-shift'
import { ShortcutTip } from './ShortcutTip'
import { W } from '../words'

type WindowTitlebarProps = {
  appearance?: 'default' | 'welcome'
  /** 왼쪽 목록이 접혀 있는지. onToggleList 가 있을 때만 접기 단추가 보인다. */
  listHidden?: boolean
  onToggleList?: () => void
  /** 뒤로·앞으로. 갈 곳이 없으면 흐리게 보이고 눌리지 않는다. */
  canGoBack?: boolean
  canGoForward?: boolean
  onBack?: () => void
  onForward?: () => void
  /** 검색 창 열기(Ctrl+K 와 같다). 목록 숨기기 단추 바로 왼쪽의 돋보기. */
  onOpenSearch?: () => void
}

export function WindowTitlebar({
  appearance = 'default',
  listHidden = false,
  onToggleList,
  onOpenSearch,
  canGoBack = false,
  canGoForward = false,
  onBack,
  onForward,
}: WindowTitlebarProps) {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    const window = getCurrentWindow()
    let disposed = false
    let unlisten: (() => void) | undefined
    const syncMaximized = () => {
      void window.isMaximized().then((value) => {
        if (!disposed) setMaximized(value)
      }).catch(() => {})
    }
    syncMaximized()
    void window.onResized(syncMaximized).then((stop) => {
      if (disposed) stop()
      else unlisten = stop
    }).catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const window = getCurrentWindow()
  // 첫 화면에서는 내용을 먼저 숨긴 다음 창 크기를 바꾼다(window-shift.ts). 다른 화면에서는 바로 바꾼다.
  const toggleMaximize = () => void prepareWindowShift().then(() => window.toggleMaximize())
  return (
    <header className={`window-titlebar ${appearance === 'welcome' ? 'window-titlebar--welcome' : ''}`} aria-label={W.windowTitlebar.aria1}>
      {onToggleList && (
        // 차례: 검색, 목록 숨기기, 뒤로, 앞으로. 검색은 Aside 앱처럼 목록 숨기기 바로 왼쪽(2026-11-16 그의 말). 목록을 닫아도 보인다.
        <div className="window-titlebar-tools">
          {onOpenSearch && (
            <ShortcutTip label={W.search} keys={['Ctrl', 'K']}>
              <button type="button" aria-label={W.search} aria-keyshortcuts="Control+K" onClick={onOpenSearch}>
                <Search size={16} strokeWidth={1.6} aria-hidden />
              </button>
            </ShortcutTip>
          )}
          <ShortcutTip label={listHidden ? W.windowTitlebar.label1 : W.windowTitlebar.label2} keys={['Ctrl', '[']}>
            <button
              type="button"
              aria-label={listHidden ? W.windowTitlebar.label1 : W.windowTitlebar.label2}
              aria-keyshortcuts="Control+BracketLeft"
              aria-pressed={!listHidden}
              onClick={onToggleList}
            >
              <PanelLeft size={16} strokeWidth={1.6} aria-hidden />
            </button>
          </ShortcutTip>
          {onBack && (
            <ShortcutTip label={W.tip.back} keys={['Alt', '←']}>
              <button
                type="button"
                aria-label={W.tip.back}
                aria-keyshortcuts="Alt+ArrowLeft"
                disabled={!canGoBack}
                onClick={onBack}
              >
                <ArrowLeft size={16} strokeWidth={1.6} aria-hidden />
              </button>
            </ShortcutTip>
          )}
          {onForward && (
            <ShortcutTip label={W.tip.forward} keys={['Alt', '→']}>
              <button
                type="button"
                aria-label={W.tip.forward}
                aria-keyshortcuts="Alt+ArrowRight"
                disabled={!canGoForward}
                onClick={onForward}
              >
                <ArrowRight size={16} strokeWidth={1.6} aria-hidden />
              </button>
            </ShortcutTip>
          )}
        </div>
      )}
      <div
        className="window-titlebar-drag"
        data-tauri-drag-region
        onMouseDown={(event) => {
          // 두 번 누르기도 창 단추와 같은 길로 보낸다. 그대로 두면 Tauri가 내용을 숨길 틈 없이 바로 창을 바꾼다.
          if (event.button !== 0 || event.detail !== 2) return
          event.preventDefault()
          event.stopPropagation()
          toggleMaximize()
        }}
      />
      <div className="window-titlebar-controls">
        <button type="button" title={W.windowTitlebar.tip1} aria-label={W.windowTitlebar.tip1} onClick={() => void window.minimize()}>
          <Minus size={15} strokeWidth={1.5} aria-hidden />
        </button>
        <button type="button" title={maximized ? W.windowTitlebar.tip2 : W.windowTitlebar.tip3} aria-label={maximized ? W.windowTitlebar.tip2 : W.windowTitlebar.tip3} onClick={toggleMaximize}>
          {maximized ? <Copy size={13} strokeWidth={1.5} aria-hidden /> : <Square size={13} strokeWidth={1.5} aria-hidden />}
        </button>
        <button className="window-titlebar-close" type="button" title={W.windowTitlebar.tip4} aria-label={W.windowTitlebar.tip4} onClick={() => void window.close()}>
          <X size={16} strokeWidth={1.5} aria-hidden />
        </button>
      </div>
    </header>
  )
}
