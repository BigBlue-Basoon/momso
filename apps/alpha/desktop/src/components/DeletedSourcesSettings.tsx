import { useCallback, useEffect, useRef, useState } from 'react'
import { RotateCcw, Trash2 } from 'lucide-react'
import { archiveErrorMessage } from '../archive'
import { listDeletedSources, previewSourceDelete, purgeSource, restoreTrashedSource, type DeletedSource, type DeletedSourceCursor } from '../source-assets'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

const D = W.deletedSources
/** 완전히 삭제를 눌렀지만 이 PC 의 것(local)이나 원격의 사본(remote)을 아직 다 지우지 못한 줄. 되돌리기는 없고, 완전히 삭제를 다시 누르면 다시 시도한다. */
const isRetry = (state: string) => state === 'local_retry_pending' || state === 'remote_retry_pending'
const retryState = (result: unknown) => {
  const state = (result as { state?: unknown } | null | undefined)?.state
  return typeof state === 'string' && isRetry(state) ? state : null
}
const dateFormat = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
function when(value: string | null | undefined) {
  if (!value) return null
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : dateFormat.format(date)
}

/**
 * 설정의 "삭제한 원자료"(2026-11-17 본부 결정): "보관된 채팅방"과 똑같은 꼴(같은 부품·같은 글의 꼴).
 * 알림 줄의 "되돌리기"를 놓쳐도 여기서 되돌리거나 완전히 삭제한다.
 * 줄: 파일 이름, 삭제한 때, 쓰던 곳 수. 올리면 되돌리기와 완전히 삭제(휴지통). 완전히 삭제는 한 번 묻는다.
 * 끝까지 내리면 다음 40개를 받는다.
 */
export function DeletedSourcesSettings({ onChanged }: { onChanged?: () => void }) {
  const [items, setItems] = useState<DeletedSource[] | null>(null)
  const [next, setNext] = useState<DeletedSourceCursor | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [working, setWorking] = useState<string | null>(null)
  const [removing, setRemoving] = useState<DeletedSource | null>(null)
  const loadingMore = useRef(false)
  const end = useRef<HTMLDivElement>(null)

  const refresh = useCallback(() => {
    listDeletedSources()
      .then((page) => { setItems(page.items); setNext(page.hasMore ? page.nextCursor : null); setError(null) })
      .catch((reason) => { setItems([]); setNext(null); setError(archiveErrorMessage(reason, D.loadFailed)) })
  }, [])
  useEffect(() => { refresh() }, [refresh])

  // 목록 끝이 보이면 다음 쪽을 받는다(새 단추 없이).
  useEffect(() => {
    if (!next || !end.current) return
    const target = end.current
    const observer = new IntersectionObserver(([entry]) => {
      if (!entry.isIntersecting || loadingMore.current) return
      loadingMore.current = true
      listDeletedSources(next)
        .then((page) => {
          setItems((current) => {
            const seen = new Set((current ?? []).map((item) => item.id))
            return [...(current ?? []), ...page.items.filter((item) => !seen.has(item.id))]
          })
          setNext(page.hasMore ? page.nextCursor : null)
        })
        .catch(() => setNext(null))
        .finally(() => { loadingMore.current = false })
    })
    observer.observe(target)
    return () => observer.disconnect()
  }, [next])

  async function run(id: string, action: () => Promise<unknown>, failed: string) {
    setWorking(id)
    setError(null)
    try {
      const result = await action()
      // 완전히 삭제가 다 끝나지 못했으면(다시 시도할 수 있음) 줄을 남기고 그 상태로 바꾼다. 그 밖에는 목록에서 뺀다.
      const retry = retryState(result)
      setItems((current) => retry
        ? current?.map((item) => item.id === id ? { ...item, deletionState: retry, canRestore: false, canPurge: true } : item) ?? current
        : current?.filter((item) => item.id !== id) ?? current)
      onChanged?.()
    } catch (reason) {
      setError(archiveErrorMessage(reason, failed))
    }
    setWorking(null)
  }

  // 완전히 삭제는 쓰는 곳을 다시 확인한 지문(planDigest)과 함께 보낸다(뒤쪽 계약).
  async function purge(id: string) {
    const preview = await previewSourceDelete(id)
    return purgeSource(id, preview.planDigest)
  }

  // 목록에는 아직 무언가 할 수 있는 것만 온다. 이름은 원격까지 다 지워질 때 지워지므로(그때는 목록에서 빠짐) 그대로 보인다(2026-11-17 뒤쪽 f033fd0).
  const nameOf = (item: DeletedSource) => item.originalFileName || W.zetPanel.text57(W.rawMaterial)

  return (
    <div className="settings-page">
      <header className="settings-page-heading">
        <h2>{D.settingsTitle}</h2>
        <p>{D.settingsIntro}</p>
      </header>
      {error && <p className="archived-error" role="alert">{error}</p>}
      {items === null && !error && <p className="archived-empty" role="status">{D.loading}</p>}
      {items && items.length === 0 && !error && <p className="archived-empty">{D.empty}</p>}
      {items && items.length > 0 && (
        <ul className="archived-list">
          {items.map((item) => {
            const title = nameOf(item)
            // 다 지우지 못해 다시 시도할 줄은 쓰던 곳 수 대신 무엇을 기다리는지 적는다(이 PC 의 것·원격의 사본을 한 글로).
            const meta = [when(item.deletedAt), isRetry(item.deletionState) ? D.retryPending : D.usedCount(item.referenceCount)].filter(Boolean).join(' · ')
            return (
              <li key={item.id} className="archived-row" aria-busy={working === item.id || undefined}>
                <span className="archived-main">
                  <span className="archived-title" title={title}>{title}</span>
                  {meta && <span className="archived-date">{meta}</span>}
                </span>
                <span className="archived-actions">
                  {item.canRestore && (
                    <button className="secondary-button" type="button" disabled={working !== null} onClick={() => void run(item.id, () => restoreTrashedSource(item.id), W.sourceDelete.restoreFailed)}>
                      <RotateCcw size={14} aria-hidden />{W.chatKeep.restore}
                    </button>
                  )}
                  {item.canPurge && (
                    <button className="secondary-button archived-remove" type="button" disabled={working !== null} aria-label={`${title} ${W.sourceDelete.purge}`} title={W.sourceDelete.purge} onClick={() => setRemoving(item)}>
                      <Trash2 size={14} aria-hidden />
                    </button>
                  )}
                </span>
              </li>
            )
          })}
        </ul>
      )}
      {next && <div ref={end} aria-hidden style={{ height: 1 }} />}
      {removing && (
        <QuestionDialog
          title={W.sourceDelete.purgeTitle(nameOf(removing))}
          text={W.sourceDelete.purgeText}
          actions={[
            { label: W.chatKeep.cancel, onSelect: () => setRemoving(null), autoFocus: true },
            { label: W.sourceDelete.purge, primary: true, onSelect: () => { const id = removing.id; setRemoving(null); void run(id, () => purge(id), D.purgeFailed) } },
          ]}
          onDismiss={() => setRemoving(null)}
        />
      )}
    </div>
  )
}
