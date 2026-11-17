import { useCallback, useEffect, useState } from 'react'
import { listMedia, type MediaItem } from '../media'
import { MediaGallery } from './MediaGallery'
import type { SourceNotice } from './SourceDelete'
import './MediaView.css'
import { W } from '../words'

/**
 * 미디어 화면: 만든 사진·동영상·소리를 새것부터 날짜별로 묶어 갤러리로 보인다(같은 높이의 줄로 꽉 채움).
 * 칸에 올리면 어느 채팅방에서, 언제, 누가 만들었는지가 드러나고, 누르면 크게 본다(정보는 늘 보임, 채팅방 이름을 누르면 그 채팅방으로).
 * 읽는 동안·없을 때·읽지 못했을 때는 문서 목록과 같은 꼴의 한 줄(2026-11-17, 하얗게 빈 화면이 되지 않게).
 */
export function MediaView({ onOpenChat, onNotice }: { onOpenChat?: (chatId: string) => void; onNotice?: SourceNotice }) {
  const [items, setItems] = useState<MediaItem[] | null>(null)
  const [failed, setFailed] = useState(false)

  const load = useCallback(() => {
    setFailed(false)
    setItems(null)
    listMedia().then(setItems).catch(() => { setItems([]); setFailed(true) })
  }, [])
  useEffect(() => { load() }, [load])

  return (
    <section className="media-view" aria-label={W.mediaView.aria1}>
      <header className="media-header">
        <h1>{W.mediaView.aria1}</h1>
      </header>
      <div className="media-scroll">
        {failed ? (
          <div className="media-message" role="alert">
            <span>{W.mediaView.failed}</span>
            <button className="secondary-button" type="button" onClick={load}>{W.firstRun.retry}</button>
          </div>
        ) : items === null ? (
          <div className="media-message" role="status"><span>{W.mediaView.loading}</span></div>
        ) : items.length === 0 ? (
          <div className="media-message"><span>{W.mediaView.empty}</span></div>
        ) : (
          <MediaGallery items={items} rowHeight={210} groupByDay onOpenChat={onOpenChat} onNotice={onNotice} onChanged={load} />
        )}
      </div>
    </section>
  )
}
