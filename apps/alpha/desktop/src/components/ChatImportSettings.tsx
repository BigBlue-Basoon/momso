import { useState } from 'react'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { LoaderCircle } from 'lucide-react'
import { errorState } from '../backup'
import { importChatExport, type ChatImportSource } from '../chats'
import { W } from '../words'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'

const SOURCES: Array<{ id: ChatImportSource; mark: ProviderMarkId; ready: boolean }> = [
  { id: 'chatgpt', mark: 'chatgpt', ready: true },
  { id: 'claude', mark: 'claude', ready: true },
  // 제미나이 내보내기는 뒤쪽이 아직 읽지 못한다(chat_import_export 가 unsupported_format). 줄은 두되 고르기 단추는 없다.
  { id: 'gemini', mark: 'gemini', ready: false },
]

/**
 * 설정 › AI 연결 아래: 다른 AI 에서 내보낸 대화 가져오기(2026-11-16 그의 말, 가장 작게 한 벌. 그가 보고 고친다).
 * 회사마다 한 줄: 로고, 이름, 어디서 받은 파일인지 한 줄, "파일 고르기". 고르면 chat_import_export({ path, provider })로 가져오고,
 * 끝나면 몇 개를 가져왔고 몇 개는 이미 있어 건너뛰었는지 한 줄. 가져온 대화는 뒤쪽이 채팅방으로 만들어 왼쪽 목록에 생긴다(onChanged 로 다시 읽음).
 * 뒤쪽에 파일 안의 대화 수를 미리 세는 명령이 없어, 가져오기 전에 몇 개인지는 보이지 않는다.
 */
export function ChatImportSettings({ onChanged }: { onChanged: () => void }) {
  const [working, setWorking] = useState<ChatImportSource | null>(null)
  const [result, setResult] = useState<{ source: ChatImportSource; text: string; failed: boolean } | null>(null)
  const words = W.chatImport

  async function pick(source: ChatImportSource) {
    if (working) return
    const picked = await openDialog({ multiple: false, directory: false, filters: [{ name: words.fileKind, extensions: ['zip', 'json'] }] }).catch(() => null)
    if (typeof picked !== 'string') return
    setWorking(source)
    setResult(null)
    try {
      const counts = await importChatExport(picked, source)
      const parts = [words.done(counts.imported)]
      if (counts.duplicate > 0) parts.push(words.duplicate(counts.duplicate))
      if (counts.updated > 0) parts.push(words.updated(counts.updated))
      setResult({ source, text: parts.join(' '), failed: false })
      onChanged()
    } catch (error) {
      const state = errorState(error)
      setResult({ source, text: (state && words.reason[state]) || words.failed, failed: true })
    } finally {
      setWorking(null)
    }
  }

  return (
    <section className="settings-group chat-import" aria-labelledby="chat-import-title">
      <h3 id="chat-import-title">{words.title}</h3>
      <p className="chat-import-intro">{words.intro}</p>
      <ul className="provider-rows">
        {SOURCES.map((source) => (
          <li key={source.id} className={source.ready ? 'provider-row' : 'provider-row is-unavailable'}>
            <ProviderMark provider={source.mark} className="provider-row-mark" />
            <div className="provider-row-body">
              <div className="provider-row-head">
                <strong>{words.name[source.id]}</strong>
              </div>
              <p className="provider-hint">{words.hint[source.id]}</p>
              {source.ready && (
                <div className="provider-actions">
                  {working === source.id ? (
                    <span className="provider-pending" role="status"><LoaderCircle size={14} className="is-spinning" aria-hidden />{words.working}</span>
                  ) : (
                    <button className="secondary-button" type="button" disabled={working !== null} onClick={() => void pick(source.id)}>{words.pick}</button>
                  )}
                </div>
              )}
              {result?.source === source.id && (
                <p className={result.failed ? 'provider-error' : 'provider-hint chat-import-done'} role={result.failed ? 'alert' : 'status'}>{result.text}</p>
              )}
            </div>
          </li>
        ))}
      </ul>
    </section>
  )
}
