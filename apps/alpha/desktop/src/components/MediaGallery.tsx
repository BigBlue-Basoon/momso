import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AudioLines, ChevronLeft, ChevronRight, Play, X } from 'lucide-react'
import { mediaChat, mediaSrc, type MediaItem } from '../media'
import { PROVIDER_NAME } from '../ai-models'
import { W } from '../words'
import { ModelName } from './ModelName'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import './MediaGallery.css'

const MARK: Record<'codex' | 'claude' | 'gemini', ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }
const dayFormat = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric', weekday: 'short' })
const timeFormat = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' })

function duration(seconds: number | null) {
  if (seconds === null || !Number.isFinite(seconds)) return null
  const s = Math.round(seconds)
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/** 가로세로 비. 모르면 사진은 정사각, 소리는 가로로 긴 칸. */
function ratioOf(item: MediaItem) {
  if (item.width && item.height) return Math.max(0.4, Math.min(2.6, item.width / item.height))
  return item.kind === 'audio' ? 1.6 : item.kind === 'video' ? 16 / 9 : 1
}

type Row = { items: Array<{ item: MediaItem; index: number; width: number }>; height: number }

/**
 * 같은 높이의 줄로 사진을 꽉 채운다(Google 포토·Windows 사진의 격자처럼 가로세로 비를 그대로 두고 줄 끝을 맞춤).
 * 마지막 줄은 늘리지 않는다.
 */
function justify(list: Array<{ item: MediaItem; index: number }>, width: number, target: number, gap: number): Row[] {
  const rows: Row[] = []
  let current: Array<{ item: MediaItem; index: number; ratio: number }> = []
  let sum = 0
  for (const entry of list) {
    const ratio = ratioOf(entry.item)
    current.push({ ...entry, ratio })
    sum += ratio
    const natural = sum * target + gap * (current.length - 1)
    if (natural >= width) {
      const height = (width - gap * (current.length - 1)) / sum
      rows.push({ height, items: current.map((c) => ({ item: c.item, index: c.index, width: c.ratio * height })) })
      current = []
      sum = 0
    }
  }
  if (current.length > 0) rows.push({ height: target, items: current.map((c) => ({ item: c.item, index: c.index, width: c.ratio * target })) })
  return rows
}

/**
 * 누가 만들었는지: AI 면 회사 CI 와 모델 이름, 그리고 뒤쪽이 적은 how("만듦"/"첨부").
 * by·how 가 null 이면(뒤쪽이 아직 모름) 그 칸은 그리지 않는다. 화면이 짐작해 넣지 않는다(2026-11-16 옮긴 것 절).
 */
export function MediaBy({ item, small = false }: { item: MediaItem; small?: boolean }) {
  const by = item.by
  const how = item.how ? W.gallery.how[item.how] ?? null : null
  if (by?.kind !== 'ai' && !how) return null
  return (
    <span className={small ? 'media-by is-small' : 'media-by'}>
      {by?.kind === 'ai' && <ProviderMark provider={MARK[by.provider]} className="media-by-mark" onDark />}
      {by?.kind === 'ai' && <span>{by.model ? <ModelName label={by.model} /> : PROVIDER_NAME[by.provider]}</span>}
      {how && <span className="media-by-how">{how}</span>}
    </span>
  )
}

/**
 * 사진·동영상·소리 갤러리. 미디어 화면(날짜별 묶음)과 오른쪽 패널(작은 칸)이 같이 쓴다.
 * 칸 위에 올리면 어느 채팅방에서, 언제, 누가 만들었는지가 사진을 가리지 않게 아래에 드러나고, 누르면 크게 본다.
 */
export function MediaGallery({ items, rowHeight, groupByDay, onOpenChat }: {
  items: MediaItem[]
  rowHeight: number
  groupByDay: boolean
  onOpenChat?: (chatId: string) => void
}) {
  const [openIndex, setOpenIndex] = useState<number | null>(null)
  const [width, setWidth] = useState(0)
  const box = useRef<HTMLDivElement>(null)
  const gap = rowHeight < 140 ? 3 : 4

  useLayoutEffect(() => {
    if (!box.current) return
    const observer = new ResizeObserver(([entry]) => setWidth(Math.floor(entry.contentRect.width)))
    observer.observe(box.current)
    return () => observer.disconnect()
  }, [])

  const sorted = useMemo(() => [...items].sort((a, b) => (b.at ?? b.createdAt).localeCompare(a.at ?? a.createdAt)), [items])
  const groups = useMemo(() => {
    const out: Array<{ day: string | null; entries: Array<{ item: MediaItem; index: number }> }> = []
    sorted.forEach((item, index) => {
      const day = groupByDay ? dayFormat.format(new Date(item.createdAt)) : null
      const last = out[out.length - 1]
      if (last && last.day === day) last.entries.push({ item, index })
      else out.push({ day, entries: [{ item, index }] })
    })
    return out
  }, [sorted, groupByDay])

  return (
    <div ref={box} className={rowHeight < 140 ? 'media-gallery is-compact' : 'media-gallery'} style={{ ['--media-gap' as string]: `${gap}px` }}>
      {width > 0 && groups.map((group) => (
        <section key={group.day ?? 'all'} className="media-day" aria-label={group.day ?? undefined}>
          {group.day && <h2>{group.day}</h2>}
          {justify(group.entries, width, rowHeight, gap).map((row, rowIndex) => (
            <div key={rowIndex} className="media-row" style={{ height: row.height, ['--row-height' as string]: `${Math.round(row.height)}px` }}>
              {row.items.map(({ item, index, width: tileWidth }) => (
                <MediaTile key={item.id} item={item} width={tileWidth} onOpen={() => setOpenIndex(index)} />
              ))}
            </div>
          ))}
        </section>
      ))}
      {openIndex !== null && sorted[openIndex] && (
        <MediaLightbox
          items={sorted}
          index={openIndex}
          onIndex={setOpenIndex}
          onClose={() => setOpenIndex(null)}
          onOpenChat={onOpenChat ? (id) => { setOpenIndex(null); onOpenChat(id) } : undefined}
        />
      )}
    </div>
  )
}

function MediaTile({ item, width, onOpen }: { item: MediaItem; width: number; onOpen: () => void }) {
  const chat = mediaChat(item)
  const when = timeFormat.format(new Date(item.at ?? item.createdAt))
  const label = item.title ?? (item.kind === 'video' ? W.mediaView.aria2 : item.kind === 'audio' ? W.gallery.audio : W.mediaView.aria3)
  const picture = item.kind === 'video' ? item.posterPath : item.kind === 'image' ? item.path : null
  return (
    <button className={`media-tile is-${item.kind}`} type="button" style={{ width }} onClick={onOpen} aria-label={label}>
      {picture ? (
        <img src={mediaSrc(picture) ?? undefined} alt="" loading="lazy" decoding="async" draggable={false} />
      ) : (
        <span className="media-audio-face" aria-hidden>
          <AudioLines size={22} />
          <span>{item.title}</span>
        </span>
      )}
      {(item.kind === 'video' || item.kind === 'audio') && duration(item.durationSec) && (
        <span className="media-length">
          {item.kind === 'video' && <Play size={10} fill="currentColor" aria-hidden />}
          {duration(item.durationSec)}
        </span>
      )}
      <span className="media-info" aria-hidden>
        {chat?.title && <span className="media-info-chat">{chat.title}</span>}
        <span className="media-info-line">
          <MediaBy item={item} small />
          <time dateTime={item.at ?? item.createdAt}>{when}</time>
        </span>
      </span>
    </button>
  )
}

function MediaLightbox({ items, index, onIndex, onClose, onOpenChat }: {
  items: MediaItem[]
  index: number
  onIndex: (index: number) => void
  onClose: () => void
  onOpenChat?: (chatId: string) => void
}) {
  const item = items[index]
  const chat = mediaChat(item)
  const go = useCallback((step: number) => onIndex((index + step + items.length) % items.length), [index, items.length, onIndex])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.stopPropagation(); onClose() }
      if (event.key === 'ArrowLeft' && !event.altKey) go(-1)
      if (event.key === 'ArrowRight' && !event.altKey) go(1)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [go, onClose])

  return createPortal(
    <div className="media-lightbox" role="dialog" aria-modal="true" aria-label={item.title ?? W.mediaView.aria4} onClick={onClose}>
      <figure key={item.id} className="media-lightbox-body" onClick={(event) => event.stopPropagation()}>
        {item.kind === 'video' ? (
          <video src={mediaSrc(item.path) ?? undefined} poster={mediaSrc(item.posterPath) ?? undefined} controls />
        ) : item.kind === 'audio' ? (
          <div className="media-lightbox-audio">
            <AudioLines size={40} aria-hidden />
            <audio src={mediaSrc(item.path) ?? undefined} controls />
          </div>
        ) : (
          <img src={mediaSrc(item.path) ?? undefined} alt={item.title ?? ''} />
        )}
        <figcaption className="media-lightbox-caption">
          {item.title && <strong>{item.title}</strong>}
          <span className="media-lightbox-meta">
            {chat && (onOpenChat ? (
              <button className="media-lightbox-chat" type="button" title={W.gallery.openChat} onClick={() => onOpenChat(chat.id)}>
                {chat.title ?? W.chatRoom}
              </button>
            ) : chat.title && <span>{chat.title}</span>)}
            <MediaBy item={item} />
            <time dateTime={item.at ?? item.createdAt}>{timeFormat.format(new Date(item.at ?? item.createdAt))}</time>
          </span>
        </figcaption>
      </figure>
      {items.length > 1 && (
        <>
          <button className="media-lightbox-nav is-prev" type="button" aria-label={W.mediaView.aria5} onClick={(event) => { event.stopPropagation(); go(-1) }}>
            <ChevronLeft size={22} aria-hidden />
          </button>
          <button className="media-lightbox-nav is-next" type="button" aria-label={W.mediaView.aria6} onClick={(event) => { event.stopPropagation(); go(1) }}>
            <ChevronRight size={22} aria-hidden />
          </button>
        </>
      )}
      <button className="media-lightbox-close" type="button" aria-label={W.mediaView.aria7} onClick={onClose}>
        <X size={20} aria-hidden />
      </button>
    </div>,
    document.body,
  )
}
