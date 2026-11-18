import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AudioLines, ChevronLeft, ChevronRight, Copy, Download, Layers, MessageSquare, Play, RefreshCw, X } from 'lucide-react'
import { listMediaVersions, mediaChat, mediaSrc, saveMediaAs, type MediaItem } from '../media'
import { PROVIDER_NAME } from '../ai-models'
import { W } from '../words'
import { SourceDeleteDialog, type SourceNotice } from './SourceDelete'
import { ModelName } from './ModelName'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import { usePanelBrowserHold } from '../panel-browser'
import { observeSize } from '../resize-later'
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

const whenOf = (item: MediaItem) => item.at ?? item.createdAt

/**
 * 생성물 하나 = 같은 묶음(lineageId)의 버전들(2026-11-18, AI 생성 도구들처럼: 같은 프롬프트로 다시 만든 것·고친 것을 한 칸에).
 * 묶음 값이 없으면 한 장이 한 생성물이다. 버전은 차례(version, 없으면 만든 때)대로, 칸에는 마지막 버전이 보인다.
 */
export type Creation = { key: string; versions: MediaItem[]; cover: MediaItem }

export function toCreations(items: MediaItem[]): Creation[] {
  const groups = new Map<string, MediaItem[]>()
  for (const item of items) {
    const key = item.lineageId ? `lineage:${item.lineageId}` : `item:${item.id}`
    const list = groups.get(key)
    if (list) list.push(item)
    else groups.set(key, [item])
  }
  const out: Creation[] = []
  for (const [key, list] of groups) {
    const versions = [...list].sort((a, b) => (a.version ?? 0) - (b.version ?? 0) || whenOf(a).localeCompare(whenOf(b)))
    out.push({ key, versions, cover: versions[versions.length - 1] })
  }
  return out.sort((a, b) => whenOf(b.cover).localeCompare(whenOf(a.cover)))
}

type Row = { items: Array<{ creation: Creation; index: number; width: number }>; height: number }

/**
 * 같은 높이의 줄로 칸을 꽉 채운다(Google 포토·Windows 사진의 격자처럼 가로세로 비를 그대로 두고 줄 끝을 맞춤).
 * 마지막 줄은 늘리지 않는다.
 */
function justify(list: Array<{ creation: Creation; index: number }>, width: number, target: number, gap: number): Row[] {
  const rows: Row[] = []
  let current: Array<{ creation: Creation; index: number; ratio: number }> = []
  let sum = 0
  for (const entry of list) {
    const ratio = ratioOf(entry.creation.cover)
    current.push({ ...entry, ratio })
    sum += ratio
    const natural = sum * target + gap * (current.length - 1)
    if (natural >= width) {
      const height = (width - gap * (current.length - 1)) / sum
      rows.push({ height, items: current.map((c) => ({ creation: c.creation, index: c.index, width: c.ratio * height })) })
      current = []
      sum = 0
    }
  }
  if (current.length > 0) rows.push({ height: target, items: current.map((c) => ({ creation: c.creation, index: c.index, width: c.ratio * target })) })
  return rows
}

/**
 * 누가 만들었는지: AI 면 회사 CI 와 모델 이름, 그리고 뒤쪽이 적은 how("생성"/"수정"/"첨부").
 * by·how 가 null 이면(뒤쪽이 아직 모름) 그 칸은 그리지 않는다. 화면이 짐작해 넣지 않는다(2026-11-16 옮긴 것 절).
 */
export function MediaBy({ item, small = false, onDark = true }: { item: MediaItem; small?: boolean; onDark?: boolean }) {
  const by = item.by
  const how = item.how ? W.gallery.how[item.how] ?? null : null
  if (by?.kind !== 'ai' && !how) return null
  return (
    <span className={small ? 'media-by is-small' : 'media-by'}>
      {by?.kind === 'ai' && <ProviderMark provider={MARK[by.provider]} className="media-by-mark" onDark={onDark} />}
      {by?.kind === 'ai' && <span>{by.model ? <ModelName label={by.model} /> : PROVIDER_NAME[by.provider]}</span>}
      {how && <span className="media-by-how">{how}</span>}
    </span>
  )
}

/** 지울 수 있는 것: 몸소 폴더 안의 원자료(로컬 경로)만. 웹 주소로 보이는 것(개발 앱의 예시 등)은 지우기를 두지 않는다. */
const deletable = (item: MediaItem) => !/^(https?:|data:|blob:)/.test(item.path)

/**
 * 사진과 동영상 갤러리(사진과 동영상 화면). 날짜별로 묶고, 생성물마다 한 칸(버전이 여럿이면 칸 모서리에 수).
 * 칸에 올리면 어느 채팅방에서, 언제, 누가 만들었는지가 사진을 가리지 않게 아래에 드러나고, 누르면 크게 본다.
 */
export function MediaGallery({ items, rowHeight, groupByDay, onOpenChat, onNotice, onChanged, onRegenerate }: {
  items: MediaItem[]
  rowHeight: number
  groupByDay: boolean
  onOpenChat?: (chatId: string) => void
  /** 있으면 크게 보기에 "삭제"를 둔다(잘못 첨부한 원자료 지우기). 프롬프트를 복사했다는 알림도 이것으로. */
  onNotice?: SourceNotice
  /** 지우거나 되살린 뒤 목록을 다시 읽는다. */
  onChanged?: () => void
  /** "다시 만들기": 원래 채팅방으로 가서 프롬프트를 채우고 생성물을 붙인다(2026-11-18 그의 결정). */
  onRegenerate?: (item: MediaItem) => void
}) {
  const [openIndex, setOpenIndex] = useState<number | null>(null)
  // 지울 원자료. 묻는 창은 크게 보기 밖에 두어(창 안을 눌러도 크게 보기가 닫히지 않게) 크게 보기 위에 뜬다.
  const [deleting, setDeleting] = useState<MediaItem | null>(null)
  const [width, setWidth] = useState(0)
  const box = useRef<HTMLDivElement>(null)
  const gap = rowHeight < 140 ? 3 : 4

  useLayoutEffect(() => {
    if (!box.current) return
    // 처음 폭은 곧바로 잰다(그림이 멈춘 창에서도 칸이 그려지게). 그 뒤 바뀌는 폭은 아래 관찰로.
    setWidth(Math.floor(box.current.getBoundingClientRect().width))
    // 줄을 다시 짜면 높이가 바뀌어 스크롤 막대가 생기고 폭이 또 바뀐다. 다음 그림으로 미룬다(2026-11-18).
    return observeSize(box.current, (entry) => setWidth(Math.floor(entry.contentRect.width)))
  }, [])

  const creations = useMemo(() => toCreations(items), [items])
  const groups = useMemo(() => {
    const out: Array<{ day: string | null; entries: Array<{ creation: Creation; index: number }> }> = []
    creations.forEach((creation, index) => {
      const day = groupByDay ? dayFormat.format(new Date(whenOf(creation.cover))) : null
      const last = out[out.length - 1]
      if (last && last.day === day) last.entries.push({ creation, index })
      else out.push({ day, entries: [{ creation, index }] })
    })
    return out
  }, [creations, groupByDay])

  return (
    <div ref={box} className={rowHeight < 140 ? 'media-gallery is-compact' : 'media-gallery'} style={{ ['--media-gap' as string]: `${gap}px` }}>
      {width > 0 && groups.map((group) => (
        <section key={group.day ?? 'all'} className="media-day" aria-label={group.day ?? undefined}>
          {group.day && <h2>{group.day}</h2>}
          {justify(group.entries, width, rowHeight, gap).map((row, rowIndex) => (
            <div key={rowIndex} className="media-row" style={{ height: row.height, ['--row-height' as string]: `${Math.round(row.height)}px` }}>
              {row.items.map(({ creation, index, width: tileWidth }) => (
                <MediaTile key={creation.key} creation={creation} width={tileWidth} onOpen={() => setOpenIndex(index)} />
              ))}
            </div>
          ))}
        </section>
      ))}
      {openIndex !== null && creations[openIndex] && (
        <MediaDetail
          creations={creations}
          index={openIndex}
          onIndex={setOpenIndex}
          onClose={() => setOpenIndex(null)}
          onOpenChat={onOpenChat ? (id) => { setOpenIndex(null); onOpenChat(id) } : undefined}
          onDelete={onNotice ? (item) => setDeleting(item) : undefined}
          onNotice={onNotice}
          onRegenerate={onRegenerate ? (item) => { setOpenIndex(null); onRegenerate(item) } : undefined}
          paused={deleting !== null}
        />
      )}
      {deleting && onNotice && (
        <SourceDeleteDialog
          assetId={deleting.id}
          name={deleting.title ?? mediaLabel(deleting)}
          onClose={() => setDeleting(null)}
          onNotice={onNotice}
          onChanged={() => { setOpenIndex(null); onChanged?.() }}
        />
      )}
    </div>
  )
}

function MediaTile({ creation, width, onOpen }: { creation: Creation; width: number; onOpen: () => void }) {
  const item = creation.cover
  const chat = mediaChat(item)
  const when = timeFormat.format(new Date(whenOf(item)))
  const label = item.title ?? (item.prompt?.trim() || null) ?? mediaLabel(item)
  const picture = item.kind === 'video' ? item.posterPath : item.kind === 'image' ? item.path : null
  // 목록에는 최신 판 하나와 판 수(versionCount)만 올 수 있다(뒤쪽 C52).
  const count = Math.max(creation.versions.length, item.versionCount ?? 1)
  return (
    <button className={`media-tile is-${item.kind}`} type="button" style={{ width }} onClick={onOpen} aria-label={count > 1 ? `${label}, ${W.mediaView.versions(count)}` : label}>
      {picture ? (
        <img src={mediaSrc(picture) ?? undefined} alt="" loading="lazy" decoding="async" draggable={false} />
      ) : (
        <span className="media-audio-face" aria-hidden>
          <AudioLines size={22} />
          <span>{item.title}</span>
        </span>
      )}
      {(count > 1 || ((item.kind === 'video' || item.kind === 'audio') && duration(item.durationSec))) && (
        <span className="media-badges" aria-hidden>
          {count > 1 && <span className="media-badge"><Layers size={10} />{count}</span>}
          {(item.kind === 'video' || item.kind === 'audio') && duration(item.durationSec) && (
            <span className="media-badge">
              {item.kind === 'video' && <Play size={10} fill="currentColor" />}
              {duration(item.durationSec)}
            </span>
          )}
        </span>
      )}
      <span className="media-info" aria-hidden>
        {chat?.title && <span className="media-info-chat">{chat.title}</span>}
        <span className="media-info-line">
          <MediaBy item={item} small />
          <time dateTime={whenOf(item)}>{when}</time>
        </span>
      </span>
    </button>
  )
}

/**
 * 크게 보기(2026-11-18, AI 생성 도구들의 꼴): 왼쪽에 크게, 오른쪽 칸에 프롬프트·누가·언제·버전·채팅방·삭제를 한 번씩.
 * 왼쪽·오른쪽 화살표는 다른 생성물로, 버전은 오른쪽 칸의 작은 그림 줄에서 고른다.
 */
function MediaDetail({ creations, index, onIndex, onClose, onOpenChat, onDelete, onNotice, onRegenerate, paused = false }: {
  creations: Creation[]
  index: number
  onIndex: (index: number) => void
  onClose: () => void
  onOpenChat?: (chatId: string) => void
  onDelete?: (item: MediaItem) => void
  onNotice?: SourceNotice
  onRegenerate?: (item: MediaItem) => void
  /** 위에 묻는 창이 떠 있으면 Esc·화살표를 받지 않는다. */
  paused?: boolean
}) {
  // 화면 가득 덮으므로 열려 있는 동안 패널 브라우저를 숨긴다.
  usePanelBrowserHold()
  const base = creations[index]
  // 목록에 판이 하나만 왔는데 판 수가 더 많으면(뒤쪽 C52) 판 전체를 media_versions 로 읽는다.
  const [loaded, setLoaded] = useState<{ key: string; versions: MediaItem[] } | null>(null)
  useEffect(() => {
    const cover = base.cover
    if (base.versions.length > 1 || (cover.versionCount ?? 1) <= 1 || !cover.lineageId) return
    let alive = true
    listMediaVersions(cover)
      .then((versions) => { if (alive && versions.length > 1) setLoaded({ key: base.key, versions }) })
      .catch(() => {})
    return () => { alive = false }
  }, [base])
  const creation = loaded && loaded.key === base.key ? { ...base, versions: loaded.versions } : base
  // 처음에는 마지막 버전. 다른 생성물로 넘어가면 그 생성물의 마지막 버전으로.
  const [versionIndex, setVersionIndex] = useState(creation.versions.length - 1)
  useEffect(() => { setVersionIndex(creation.versions.length - 1) }, [creation.key, creation.versions.length])
  const item = creation.versions[Math.min(versionIndex, creation.versions.length - 1)]
  // 빈 칸·빈칸만 있는 프롬프트는 없는 것으로 본다.
  const prompt = item.prompt?.trim() || null
  const chat = mediaChat(item)
  const go = useCallback((step: number) => onIndex((index + step + creations.length) % creations.length), [index, creations.length, onIndex])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (paused) return
      if (event.key === 'Escape') { event.stopPropagation(); onClose() }
      if (event.key === 'ArrowLeft' && !event.altKey) go(-1)
      if (event.key === 'ArrowRight' && !event.altKey) go(1)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [go, onClose, paused])

  // 내려받기(media_save_as): Windows 저장 창에서 사람이 고른 자리에 새 파일로. 취소하면 아무 알림도 없다.
  const [saving, setSaving] = useState(false)
  function saveAs() {
    setSaving(true)
    saveMediaAs(item.id)
      .then((status) => { if (status === 'saved') onNotice?.(W.mediaView.saved) })
      .catch((error) => onNotice?.(W.mediaView.saveFailed[String(error)] ?? W.mediaView.saveFailed.other))
      .finally(() => setSaving(false))
  }

  function copyPrompt() {
    if (!prompt) return
    void navigator.clipboard.writeText(prompt)
      .then(() => onNotice?.(W.mediaView.copied))
      .catch(() => onNotice?.(W.mediaView.copyFailed))
  }

  return createPortal(
    <div className="media-lightbox" role="dialog" aria-modal="true" aria-label={item.title ?? W.mediaView.aria4} onClick={onClose}>
      <div key={creation.key} className="media-detail" onClick={(event) => event.stopPropagation()}>
        <figure className="media-stage">
          {item.kind === 'video' ? (
            <video key={item.id} src={mediaSrc(item.path) ?? undefined} poster={mediaSrc(item.posterPath) ?? undefined} controls />
          ) : item.kind === 'audio' ? (
            <div className="media-lightbox-audio">
              <AudioLines size={40} aria-hidden />
              <audio key={item.id} src={mediaSrc(item.path) ?? undefined} controls />
            </div>
          ) : (
            <img key={item.id} src={mediaSrc(item.path) ?? undefined} alt={item.title ?? prompt ?? ''} />
          )}
        </figure>
        <aside className="media-side">
          {item.title && <h2 className="media-side-title">{item.title}</h2>}
          {/* 도구가 알리지 않아 자동 등록된 생성물은 프롬프트가 비어 온다(뒤쪽 f42672f): 빈 칸 대신 한 줄, 복사 단추는 두지 않는다. */}
          {prompt ? (
            <div className="media-prompt">
              <p>{prompt}</p>
              <button className="media-side-button" type="button" onClick={copyPrompt}>
                <Copy size={13} aria-hidden />{W.mediaView.copyPrompt}
              </button>
            </div>
          ) : (item.by?.kind === 'ai' || item.lineageId) ? <p className="media-prompt-missing">{W.mediaView.noPrompt}</p> : null}
          <p className="media-side-meta">
            <MediaBy item={item} onDark={false} />
            <time dateTime={whenOf(item)}>{timeFormat.format(new Date(whenOf(item)))}</time>
          </p>
          {creation.versions.length > 1 && (
            <div className="media-versions" role="group" aria-label={W.mediaView.versions(creation.versions.length)}>
              {creation.versions.map((version, at) => {
                const picture = version.kind === 'video' ? version.posterPath : version.kind === 'image' ? version.path : null
                const name = W.mediaView.version(version.version ?? at + 1)
                const how = version.versionHow ? W.mediaView.versionHow[version.versionHow] : null
                return (
                  <button
                    key={version.id}
                    type="button"
                    className={version.id === item.id ? 'media-version is-current' : 'media-version'}
                    aria-pressed={version.id === item.id}
                    aria-label={how ? `${name}, ${how}` : name}
                    title={how ? `${name} · ${how}` : name}
                    disabled={!version.path}
                    onClick={() => setVersionIndex(at)}
                  >
                    {picture ? <img src={mediaSrc(picture) ?? undefined} alt="" draggable={false} /> : version.kind === 'audio' ? <AudioLines size={16} aria-hidden /> : <span className="media-version-missing" aria-hidden>—</span>}
                    <span>{version.version ?? at + 1}</span>
                  </button>
                )
              })}
            </div>
          )}
          {item.versionHow && <p className="media-version-how">{W.mediaView.version(item.version ?? versionIndex + 1)} · {W.mediaView.versionHow[item.versionHow]}</p>}
          <div className="media-side-actions">
            {/* 다시 만들기: 원래 채팅방이 있을 때만 누를 수 있다(없거나 지워졌으면 꺼 둠). */}
            {onRegenerate && (
              <button className="media-side-button is-primary" type="button" disabled={!chat} title={chat ? undefined : W.mediaView.regenerateNoChat} onClick={() => onRegenerate(item)}>
                <RefreshCw size={13} aria-hidden />{W.mediaView.regenerate}
              </button>
            )}
            {deletable(item) && item.path && item.kind !== 'audio' && (
              <button className="media-side-button" type="button" disabled={saving} onClick={saveAs}>
                <Download size={13} aria-hidden />{W.mediaView.saveAs}
              </button>
            )}
            {chat && onOpenChat && (
              <button className="media-side-button" type="button" title={W.gallery.openChat} onClick={() => onOpenChat(chat.id)}>
                <MessageSquare size={13} aria-hidden />{chat.title ?? W.chatRoom}
              </button>
            )}
            {onDelete && deletable(item) && (
              <button className="media-side-button" type="button" onClick={() => onDelete(item)}>{W.sourceDelete.trash}</button>
            )}
          </div>
        </aside>
      </div>
      {creations.length > 1 && (
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

/** 이름이 없는 것의 이름(칸의 화면 읽기 이름과 같은 말). */
function mediaLabel(item: MediaItem) {
  return item.kind === 'video' ? W.mediaView.aria2 : item.kind === 'audio' ? W.gallery.audio : W.mediaView.aria3
}
