import { useEffect, useState } from 'react'
import { listMedia, type MediaItem } from '../media'
import { MediaGallery } from './MediaGallery'
import './MediaView.css'
import { W } from '../words'

/**
 * 미디어 화면: 만든 사진·동영상·소리를 새것부터 날짜별로 묶어 갤러리로 보인다(같은 높이의 줄로 꽉 채움).
 * 칸에 올리면 어느 채팅방에서, 언제, 누가 만들었는지가 드러나고, 누르면 크게 본다(정보는 늘 보임, 채팅방 이름을 누르면 그 채팅방으로).
 * 비어 있으면 아무것도 그리지 않는다.
 */
export function MediaView({ onOpenChat }: { onOpenChat?: (chatId: string) => void }) {
  const [items, setItems] = useState<MediaItem[] | null>(null)

  useEffect(() => {
    listMedia().then(setItems).catch(() => setItems([]))
  }, [])

  return (
    <section className="media-view" aria-label={W.mediaView.aria1}>
      <header className="media-header">
        <h1>{W.mediaView.aria1}</h1>
      </header>
      <div className="media-scroll">
        {items && items.length > 0 && <MediaGallery items={items} rowHeight={210} groupByDay onOpenChat={onOpenChat} />}
      </div>
    </section>
  )
}
