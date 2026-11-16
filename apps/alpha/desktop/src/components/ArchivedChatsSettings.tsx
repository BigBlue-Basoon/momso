import { useCallback, useEffect, useState } from 'react'
import { RotateCcw, Trash2 } from 'lucide-react'
import { archiveErrorMessage } from '../archive'
import { deleteChat, listArchivedChats, restoreChat, type ArchivedChat } from '../chats'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

const K = W.chatKeep
const dateFormat = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
function when(value: string | null | undefined) {
  if (!value) return null
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : dateFormat.format(date)
}

/**
 * 설정의 "보관된 채팅방"(Codex 앱 설정의 Archived chats 처럼): 보관한 채팅방 목록. 줄에 올리면 오른쪽 끝에
 * 되돌리기(왼쪽 목록으로)와 영구 삭제(휴지통) 단추. 영구 삭제는 한 번 묻고 지운다. 그 밖의 것(한꺼번에 지우기 등)은 두지 않는다.
 */
export function ArchivedChatsSettings({ onChanged }: { onChanged: () => void }) {
  const [items, setItems] = useState<ArchivedChat[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [working, setWorking] = useState<string | null>(null)
  const [removing, setRemoving] = useState<ArchivedChat | null>(null)

  const refresh = useCallback(() => {
    listArchivedChats()
      .then((list) => { setItems(list); setError(null) })
      .catch((reason) => { setItems([]); setError(archiveErrorMessage(reason, K.loadFailed)) })
  }, [])
  useEffect(() => { refresh() }, [refresh])

  async function run(id: string, action: () => Promise<void>, failed: string) {
    setWorking(id)
    setError(null)
    try {
      await action()
      setItems((current) => current?.filter((item) => item.id !== id) ?? current)
      onChanged()
    } catch (reason) {
      setError(archiveErrorMessage(reason, failed))
    }
    setWorking(null)
  }

  return (
    <div className="settings-page">
      <header className="settings-page-heading">
        <h2>{K.settingsTitle}</h2>
        <p>{K.settingsIntro}</p>
      </header>
      {error && <p className="archived-error" role="alert">{error}</p>}
      {items === null && !error && <p className="archived-empty" role="status">{K.loading}</p>}
      {items && items.length === 0 && !error && <p className="archived-empty">{K.empty}</p>}
      {items && items.length > 0 && (
        <ul className="archived-list">
          {items.map((item) => {
            const title = item.title || W.chatSidebar.text1
            return (
              <li key={item.id} className="archived-row" aria-busy={working === item.id || undefined}>
                <span className="archived-main">
                  <span className="archived-title" title={title}>{title}</span>
                  {when(item.archivedAt ?? item.updatedAt) && <span className="archived-date">{when(item.archivedAt ?? item.updatedAt)}</span>}
                </span>
                <span className="archived-actions">
                  <button className="secondary-button" type="button" disabled={working !== null} onClick={() => void run(item.id, () => restoreChat(item.id), K.restoreFailed)}>
                    <RotateCcw size={14} aria-hidden />{K.restore}
                  </button>
                  <button className="secondary-button archived-remove" type="button" disabled={working !== null} aria-label={`${title} ${K.remove}`} title={K.remove} onClick={() => setRemoving(item)}>
                    <Trash2 size={14} aria-hidden />
                  </button>
                </span>
              </li>
            )
          })}
        </ul>
      )}
      {removing && (
        <QuestionDialog
          title={K.removeTitle(removing.title || W.chatSidebar.text1)}
          text={K.removeText}
          actions={[
            { label: K.cancel, onSelect: () => setRemoving(null), autoFocus: true },
            { label: K.remove, primary: true, onSelect: () => { const id = removing.id; setRemoving(null); void run(id, () => deleteChat(id), K.removeFailed) } },
          ]}
          onDismiss={() => setRemoving(null)}
        />
      )}
    </div>
  )
}
