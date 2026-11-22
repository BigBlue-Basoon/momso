import { useCallback, useEffect, useState } from 'react'
import { Download, LoaderCircle, Pin, PinOff, Puzzle } from 'lucide-react'
import { listenExtensionsChanged, listExtensions, pinnedExtensions, removeExtension, setExtensionEnabled, setExtensionPinned, type ExtensionsSection, type PanelExtension } from '../panel-extensions'
import { W } from '../words'
import { MenuItem, RowMenu } from './ChatSidebar'
import './Extensions.css'

/**
 * 확장 그림: 백엔드가 준 그 확장의 그림(S41 iconDataUrl)이 있으면 그것을 같은 자리·같은 크기에, 없거나 그리지 못하면 크롬의 확장 그림처럼 회색 칸 안의 퍼즐 조각.
 * 이름 앞 두 글자("MI")는 그에게 뜻 없는 글자로 보였다(2026-11-21 그의 말 "MI가 뭔데").
 */
export function ExtensionIcon({ large = false, src }: { large?: boolean; src?: string | null }) {
  const [broken, setBroken] = useState(false)
  useEffect(() => { setBroken(false) }, [src])
  const image = Boolean(src) && !broken
  return (
    <span className={`ext-icon${large ? ' is-large' : ''}${image ? ' has-image' : ''}`} aria-hidden>
      {image ? <img src={src as string} alt="" onError={() => setBroken(true)} /> : <Puzzle size={large ? 18 : 12} strokeWidth={1.8} />}
    </span>
  )
}

/**
 * 퍼즐 단추의 메뉴(어사이드 aside-2·3 꼴): 확장 프로그램 관리 · 확장 프로그램 가져오기 · 구분선 · 작은 회색 "설치된 확장 프로그램" · 확장 한 줄씩(글자 그림, 이름, 고정 핀, …).
 * … 의 작은 메뉴: 맨 위 흐린 이름, 삭제, 고정/고정 해제, 사용/사용 중지, 확장 프로그램 관리(계약에 없는 웹 권한 보기·팝업 검사는 두지 않는다).
 * 떠 있는 창(PanelOverlayRoot)과 떠 있는 창이 없는 화면(PanelBrowser 의 화면 안 메뉴)이 같이 쓴다.
 */
export function ExtensionsMenu({ onManage, onPinsChanged }: { onManage: (section: ExtensionsSection) => void; onPinsChanged?: () => void }) {
  const words = W.extensions
  const [list, setList] = useState<PanelExtension[] | null>(null)
  const [pins, setPins] = useState<string[]>([])
  const [problem, setProblem] = useState<string | null>(null)

  const reload = useCallback(() => {
    Promise.all([listExtensions(), pinnedExtensions().catch(() => [] as string[])])
      .then(([items, pinned]) => { setList(items); setPins(pinned); setProblem(null) })
      .catch(() => setProblem(words.loadFailed))
  }, [words])
  useEffect(() => { reload() }, [reload])
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenExtensionsChanged(reload).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [reload])

  async function pin(item: PanelExtension, pinned: boolean) {
    try {
      await setExtensionPinned(item.id, pinned)
      setPins((now) => (pinned ? [...now.filter((id) => id !== item.id), item.id] : now.filter((id) => id !== item.id)))
      onPinsChanged?.()
    } catch { setProblem(words.errors.failed) }
  }
  async function act(run: () => Promise<unknown>) {
    try { await run(); reload() } catch { setProblem(words.errors.failed); reload() }
  }

  return (
    <div className="ext-menu">
      <button className="ext-menu-item" type="button" onClick={() => onManage('all')}><Puzzle size={14} aria-hidden />{words.manageAll}</button>
      <button className="ext-menu-item" type="button" onClick={() => onManage('import')}><Download size={14} aria-hidden />{words.importMenu}</button>
      <hr className="ext-menu-rule" />
      <p className="ext-menu-label">{words.installed}</p>
      {problem && <p className="ext-menu-note" role="alert">{problem}</p>}
      {list === null && !problem && <p className="ext-menu-note" role="status"><LoaderCircle size={13} className="is-spinning" aria-hidden />{words.loading}</p>}
      {list?.length === 0 && <p className="ext-menu-note">{words.none}</p>}
      {list?.map((item) => {
        const pinned = pins.includes(item.id)
        return (
          <div key={item.id} className={`ext-menu-row${item.enabled ? '' : ' is-off'}`}>
            <ExtensionIcon src={item.iconDataUrl} />
            <span className="ext-menu-name" title={item.name}>{item.name}{!item.enabled && <span className="ext-off"> · {words.off}</span>}</span>
            <button className={`icon-button ext-pin${pinned ? ' is-pinned' : ''}`} type="button" aria-pressed={pinned} aria-label={`${pinned ? words.unpin : words.pin}: ${item.name}`} title={pinned ? words.unpin : words.pin} onClick={() => void pin(item, !pinned)}>
              {pinned ? <PinOff size={13} aria-hidden /> : <Pin size={13} aria-hidden />}
            </button>
            <RowMenu label={`${words.more}: ${item.name}`}>
              {(close) => (
                <>
                  <p className="ext-row-menu-name">{item.name}</p>
                  <MenuItem close={close} onSelect={() => void act(() => removeExtension(item.id))}>{words.remove}</MenuItem>
                  <MenuItem close={close} onSelect={() => void pin(item, !pinned)}>{pinned ? words.unpin : words.pin}</MenuItem>
                  <MenuItem close={close} onSelect={() => void act(() => setExtensionEnabled(item.id, !item.enabled))}>{item.enabled ? words.disable : words.enable}</MenuItem>
                  <MenuItem close={close} onSelect={() => onManage('all')}>{words.manageAll}</MenuItem>
                </>
              )}
            </RowMenu>
          </div>
        )
      })}
    </div>
  )
}
