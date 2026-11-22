import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { Search, X } from 'lucide-react'
import { listMedia, mediaChat, type MediaItem } from '../media'
import { MediaGallery } from './MediaGallery'
import type { SourceNotice } from './SourceDelete'
import './MediaView.css'
import { W } from '../words'

type Kind = 'all' | 'image' | 'video'

/**
 * 사진과 동영상 화면: AI 가 채팅방에서 만든 사진·동영상을 새것부터 날짜별로 묶어 갤러리로 보인다(같은 높이의 줄로 꽉 채움).
 * 생성물마다 한 칸(버전이 여럿이면 칸 모서리에 수), 누르면 크게 보고 오른쪽에 프롬프트·버전·채팅방(2026-11-18, AI 생성 도구들의 꼴).
 * 위쪽: 전체·사진·동영상 거르기와 프롬프트로 찾기.
 * 읽는 동안·없을 때·읽지 못했을 때는 문서 목록과 같은 꼴의 한 줄(2026-11-17, 하얗게 빈 화면이 되지 않게).
 * 개발 앱(import.meta.env.DEV)에서만 생성물 몇 개를 더해 보인다(dev-samples). 설치판에는 들어가지 않는다.
 */
export function MediaView({ onOpenChat, onNotice, onRegenerate, menu }: {
  onOpenChat?: (chatId: string) => void
  onNotice?: SourceNotice
  onRegenerate?: (item: MediaItem) => void
  /** 휴대폰 몸소(S75 셋째): 맨 위 줄 왼쪽의 채팅방 목록 단추(문서 화면과 같은 것). */
  menu?: ReactNode
}) {
  const [items, setItems] = useState<MediaItem[] | null>(null)
  const [failed, setFailed] = useState(false)
  const [kind, setKind] = useState<Kind>('all')
  const [query, setQuery] = useState('')

  const load = useCallback(() => {
    setFailed(false)
    setItems(null)
    const extra = import.meta.env.DEV
      ? import('../dev-samples/media-samples').then((module) => module.devSampleMedia()).catch(() => [] as MediaItem[])
      : Promise.resolve([] as MediaItem[])
    Promise.all([listMedia().then((list) => ({ list, ok: true })).catch(() => ({ list: [] as MediaItem[], ok: false })), extra])
      .then(([real, samples]) => {
        if (!real.ok && samples.length === 0) { setItems([]); setFailed(true); return }
        setItems([...real.list, ...samples])
      })
  }, [])
  useEffect(() => { load() }, [load])
  // 다시 만들기로 새 판이 생기면(앱이 momso:media-changed 를 보냄) 다시 읽는다.
  useEffect(() => {
    const onChange = () => load()
    window.addEventListener('momso:media-changed', onChange)
    return () => window.removeEventListener('momso:media-changed', onChange)
  }, [load])

  const shown = useMemo(() => {
    if (!items) return null
    const words = query.trim().toLowerCase()
    return items.filter((item) => {
      if (kind !== 'all' && item.kind !== kind) return false
      if (!words) return true
      const text = [item.prompt, item.title, mediaChat(item)?.title].filter(Boolean).join(' ').toLowerCase()
      return text.includes(words)
    })
  }, [items, kind, query])

  const kinds: Array<[Kind, string]> = [['all', W.mediaView.all], ['image', W.mediaView.aria3], ['video', W.mediaView.aria2]]
  return (
    <section className="media-view" aria-label={W.mediaView.aria1}>
      <header className="media-header">
        {menu}
        <h1>{W.mediaView.aria1}</h1>
        {items && items.length > 0 && (
          <div className="media-tools">
            <div className="media-kinds" role="radiogroup" aria-label={W.mediaView.kindLabel}>
              {kinds.map(([id, name]) => (
                <button key={id} type="button" role="radio" aria-checked={kind === id} className={kind === id ? 'is-active' : undefined} onClick={() => setKind(id)}>{name}</button>
              ))}
            </div>
            <label className="media-search">
              <Search size={14} aria-hidden />
              <input value={query} placeholder={W.mediaView.searchPlaceholder} aria-label={W.mediaView.searchPlaceholder} onChange={(event) => setQuery(event.target.value)} />
              {query && <button type="button" aria-label={W.mediaView.clearSearch} onClick={() => setQuery('')}><X size={13} aria-hidden /></button>}
            </label>
          </div>
        )}
      </header>
      <div className="media-scroll">
        {failed ? (
          <div className="media-message" role="alert">
            <span>{W.mediaView.failed}</span>
            <button className="secondary-button" type="button" onClick={load}>{W.firstRun.retry}</button>
          </div>
        ) : shown === null ? (
          <div className="media-message" role="status"><span>{W.mediaView.loading}</span></div>
        ) : items && items.length === 0 ? (
          <div className="media-message"><span>{W.mediaView.empty}</span></div>
        ) : shown.length === 0 ? (
          <div className="media-message"><span>{W.mediaView.noMatch}</span></div>
        ) : (
          <MediaGallery items={shown} rowHeight={210} groupByDay onOpenChat={onOpenChat} onNotice={onNotice} onChanged={load} onRegenerate={onRegenerate} />
        )}
      </div>
    </section>
  )
}
