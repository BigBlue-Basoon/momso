import { useState } from 'react'
import { isSelectionExpired, selectLocalPaths } from '../runtime'
import { LoaderCircle } from 'lucide-react'
import { errorState } from '../backup'
import { cancelChatImport, importChatExportWithProgress, previewChatExport, type ChatImportPreview, type ChatImportSource } from '../chats'
import { W } from '../words'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'

const SOURCES: Array<{ id: ChatImportSource; mark: ProviderMarkId; ready: boolean }> = [
  { id: 'chatgpt', mark: 'chatgpt', ready: true },
  { id: 'claude', mark: 'claude', ready: true },
  // 제미나이: 2026-11-17 뒤쪽이 구글 데이터 내보내기의 JSON 과 zip 을 읽는다(HTML 이면 gemini_json_required, 64MiB 넘으면 gemini_file_too_large).
  { id: 'gemini', mark: 'gemini', ready: true },
]

type Step =
  | { kind: 'counting'; source: ChatImportSource }
  | { kind: 'ready'; source: ChatImportSource; path: string; preview: ChatImportPreview }
  | { kind: 'importing'; source: ChatImportSource; requestId: string; total: number; processed: number; cancelling: boolean }
  | { kind: 'result'; source: ChatImportSource; text: string; failed: boolean }

const dayFormat = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
function day(value: string | null) {
  if (!value) return null
  const ms = Date.parse(value)
  return Number.isFinite(ms) ? dayFormat.format(new Date(ms)) : null
}

/**
 * 설정 › AI 연결 아래: 다른 AI 에서 내보낸 대화 가져오기(2026-11-16 그의 말, 가장 작게 한 벌. 그가 보고 고친다).
 * 회사마다 한 줄: 로고, 이름, 어디서 받은 파일인지 한 줄, "파일 선택".
 * 파일을 고르면 먼저 센다(chat_import_export_preview): 대화 몇 개, 그 가운데 이미 가져온 것, 읽을 수 없는 것, 언제부터 언제까지.
 * "가져오기"를 누르면 chat_import_export_progress 로 가져오며 얼마나 됐는지 보이고(chat-import-progress) 그만둘 수 있다.
 * 그만두면 뒤쪽이 그 가져오기 전체를 되돌린다. 끝나면 몇 개를 가져왔는지 한 줄. 가져온 대화는 왼쪽 목록에 채팅방으로 생긴다(onChanged).
 */
export function ChatImportSettings({ onChanged }: { onChanged: () => void }) {
  const [step, setStep] = useState<Step | null>(null)
  const words = W.chatImport
  const busy = step?.kind === 'counting' || step?.kind === 'importing'

  function failure(source: ChatImportSource, error: unknown) {
    // 고른 지 5분이 지나 뒤쪽이 거절하면(selection_required) 다시 고르라는 한 줄.
    if (isSelectionExpired(error)) { setStep({ kind: 'result', source, text: W.selectAgain, failed: true }); return }
    const state = errorState(error)
    setStep({ kind: 'result', source, text: (state && words.reason[state]) || words.failed, failed: true })
  }

  async function pick(source: ChatImportSource) {
    if (busy) return
    // 고르는 창은 뒤쪽이 연다(select_local_paths conversation: JSON·JSONL·ZIP). 한 개만 쓴다. 취소하면 빈 배열.
    const picked = (await selectLocalPaths('conversation').catch(() => [] as string[]))[0]
    if (typeof picked !== 'string') return
    setStep({ kind: 'counting', source })
    try {
      const preview = await previewChatExport(picked, source)
      setStep({ kind: 'ready', source, path: picked, preview })
    } catch (error) {
      failure(source, error)
    }
  }

  async function start(source: ChatImportSource, path: string, preview: ChatImportPreview) {
    const requestId = `import-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    setStep({ kind: 'importing', source, requestId, total: preview.conversations, processed: 0, cancelling: false })
    try {
      const counts = await importChatExportWithProgress(requestId, path, source, (processed) => {
        setStep((current) => (current?.kind === 'importing' && current.requestId === requestId ? { ...current, processed } : current))
      })
      const parts = counts.needsJson ? [words.geminiJsonGuide] : [words.done(counts.imported)]
      if (counts.duplicate > 0) parts.push(words.duplicate(counts.duplicate))
      if (counts.updated > 0) parts.push(words.updated(counts.updated))
      if (counts.grouped && !counts.needsJson && counts.imported > 0) parts.push(words.geminiGrouped)
      setStep({ kind: 'result', source, text: parts.join(' '), failed: false })
      onChanged()
    } catch (error) {
      if (errorState(error) === 'cancelled') setStep({ kind: 'result', source, text: words.cancelled, failed: false })
      else failure(source, error)
    }
  }

  async function stop(requestId: string) {
    setStep((current) => (current?.kind === 'importing' ? { ...current, cancelling: true } : current))
    await cancelChatImport(requestId).catch(() => {})
  }

  function previewLines(preview: ChatImportPreview) {
    if (preview.needsJson) return [words.geminiJsonGuide]
    if (preview.conversations === 0) return [words.previewEmpty]
    const lines = [words.preview(preview.conversations)]
    if (preview.alreadyImported > 0) lines.push(words.previewAlready(preview.alreadyImported))
    if (preview.unreadable > 0) lines.push(words.previewUnreadable(preview.unreadable))
    const from = day(preview.oldest)
    const to = day(preview.newest)
    if (from && to) lines.push(from === to ? words.previewDay(from) : words.previewRange(from, to))
    if (preview.grouped) lines.push(words.geminiGrouped)
    return lines
  }

  return (
    <section className="settings-group chat-import" aria-labelledby="chat-import-title">
      <h3 id="chat-import-title">{words.title}</h3>
      <p className="chat-import-intro">{words.intro}</p>
      <ul className="provider-rows">
        {SOURCES.map((source) => {
          const mine = step?.source === source.id ? step : null
          return (
            <li key={source.id} className={source.ready ? 'provider-row' : 'provider-row is-unavailable'}>
              <ProviderMark provider={source.mark} className="provider-row-mark" />
              <div className="provider-row-body">
                <div className="provider-row-head">
                  <strong>{words.name[source.id]}</strong>
                </div>
                <p className="provider-hint">{words.hint[source.id]}</p>
                {source.ready && (
                  <div className="provider-actions">
                    {mine?.kind === 'counting' ? (
                      <span className="provider-pending" role="status"><LoaderCircle size={14} className="is-spinning" aria-hidden />{words.counting}</span>
                    ) : mine?.kind === 'importing' ? (
                      <>
                        <span className="provider-pending" role="status">
                          <LoaderCircle size={14} className="is-spinning" aria-hidden />
                          {mine.cancelling ? words.cancelling : mine.total > 0 ? words.progress(Math.min(mine.processed, mine.total), mine.total) : words.working}
                        </span>
                        {/* 단추가 바뀌면 포커스가 빠지므로 지금 할 수 있는 단추로 옮긴다(키보드만으로 쓸 때). */}
                        <button key="stop" className="secondary-button" type="button" autoFocus disabled={mine.cancelling} onClick={() => void stop(mine.requestId)}>{words.cancel}</button>
                      </>
                    ) : mine?.kind === 'ready' ? (
                      <>
                        {mine.preview.conversations > 0 && (
                          <button className="confirm-button" type="button" autoFocus onClick={() => void start(source.id, mine.path, mine.preview)}>{words.start}</button>
                        )}
                        <button key="close" className="secondary-button" type="button" autoFocus={mine.preview.conversations === 0} onClick={() => setStep(null)}>{words.cancel}</button>
                      </>
                    ) : (
                      <button className="secondary-button" type="button" autoFocus={mine?.kind === 'result'} disabled={busy} onClick={() => void pick(source.id)}>{words.pick}</button>
                    )}
                  </div>
                )}
                {mine?.kind === 'ready' && (
                  <p className="provider-hint chat-import-done" role="status">{previewLines(mine.preview).join(' ')}</p>
                )}
                {mine?.kind === 'result' && (
                  <p className={mine.failed ? 'provider-error' : 'provider-hint chat-import-done'} role={mine.failed ? 'alert' : 'status'}>{mine.text}</p>
                )}
              </div>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
