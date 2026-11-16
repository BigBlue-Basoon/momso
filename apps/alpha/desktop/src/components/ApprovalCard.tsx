import { W } from '../words'
import { useEffect, useState } from 'react'
import { Check } from 'lucide-react'
import { findZetId, readZet } from '../archive'
import { PROVIDER_NAME, type AiProvider } from '../ai-models'
import { CHOICE_DONE, CHOICE_LABEL, approvalChange, orderedChoices, type ApprovalChange, type ApprovalChoice, type ApprovalRequest } from '../approvals'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import './ApprovalCard.css'

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }

export type ApprovalItem = {
  request: ApprovalRequest
  /** 물음이 온 대화. */
  chatId: string | null
  /** 물음이 왔을 때 그 대화의 말 수. 답한 뒤 남는 한 줄을 그 자리에 둔다. */
  at: number
  status: 'pending' | 'sending' | 'answered'
  choice?: ApprovalChoice
  error?: string | null
}

function providerName(provider: string) {
  return PROVIDER_NAME[provider as AiProvider] ?? provider
}

function fileName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

/**
 * 물음의 글에서 파일 경로와 명령을 읽는다. 코덱스는 경로·명령 글 그대로, 클로드는 "도구: {입력 JSON}"(chat_claude.rs)으로 온다.
 * JSON 이면 그 안의 file_path·path·notebook_path, command 칸을 읽고, 아니면 받은 글 그대로.
 */
function readDescription(description: string): { path: string | null; command: string | null } {
  const match = /^[A-Za-z][\w-]*:\s*(\{[\s\S]*\})\s*$/.exec(description)
  if (!match) return { path: null, command: null }
  try {
    const input = JSON.parse(match[1]) as Record<string, unknown>
    const text = (value: unknown) => (typeof value === 'string' && value.trim() ? value : null)
    return { path: text(input.file_path) ?? text(input.path) ?? text(input.notebook_path), command: text(input.command) }
  } catch {
    return { path: null, command: null }
  }
}

/** 보관함에 이미 있는 글을 바꾸려는 것이면 그 글의 제목을 읽는다(파일 이름 대신 보인다). 새로 만드는 것은 아직 없으니 읽지 않는다. */
function useZetTitle(request: ApprovalRequest, path: string | null, change: ApprovalChange | null) {
  const [title, setTitle] = useState<string | null>(null)
  useEffect(() => {
    if (request.kind !== 'file_change' || change === 'create') return
    const id = findZetId(path ?? request.description)
    if (!id) return
    let current = true
    readZet(id).then((doc) => { if (current) setTitle(doc.head.title || null) }).catch(() => {})
    return () => { current = false }
  }, [request.kind, request.description, path, change])
  return title
}

function headline(request: ApprovalRequest, title: string | null, change: ApprovalChange | null, count: number, place: 'file' | 'folder' | 'none') {
  const name = providerName(request.provider)
  if (request.kind === 'command') return W.approvalCard.text1(name)
  if (request.kind !== 'file_change') return W.approvalCard.text4(name)
  if (count > 1) return W.approvalCard.many(name, count)
  if (place === 'folder') return W.approvalCard.inFolder(name)
  if (place === 'none') return W.approvalCard.someFile(name)
  if (change === 'edit') return title ? W.approvalCard.text2(name) : W.approvalCard.text3(name)
  const words = W.approvalCard.change[change ?? 'unknown']
  return title ? words.doc(name) : words.file(name)
}

/**
 * 대화 안에서 허락을 묻는 물음. Claude 앱의 대화 안 물음 상자처럼 대화 흐름 끝에 둥근 상자로 뜨고,
 * 무엇을 하려는지와 답 단추(그 AI 가 받는 것만)를 보인다. 답하면 고른 것을 적은 한 줄로 줄어 대화에 남는다.
 */
export function ApprovalCard({ item, onAnswer }: { item: ApprovalItem; onAnswer: (choice: ApprovalChoice) => void }) {
  const { request } = item
  const targets = request.kind === 'file_change' ? request.targets ?? [] : []
  const change = approvalChange(request)
  const read = readDescription(request.description)
  const path = targets[0]?.path ?? read.path ?? request.description
  // 코덱스는 파일 목록이 없으면 허락할 폴더(grantRoot)를 글로 보낸다(chat_codex.rs). 그것도 없으면 무엇인지 적지 않는다.
  const place: 'file' | 'folder' | 'none' = request.kind !== 'file_change' || targets.length > 0 || read.path
    ? 'file'
    : !request.description.trim() ? 'none' : request.provider === 'codex' ? 'folder' : 'file'
  const title = useZetTitle(request, targets.length > 1 ? null : path, change)
  const command = read.command ?? request.description
  const subject = request.kind === 'file_change'
    ? (targets.length > 1 ? targets.map((target) => fileName(target.path)).join(', ') : title ?? fileName(path))
    : request.kind === 'command' ? command : request.description

  if (item.status === 'answered' && item.choice) {
    return (
      <p className="approval-done">
        <Check size={13} aria-hidden />
        <span>{CHOICE_DONE[item.choice]}</span>
        <span className="approval-done-subject">{subject}</span>
      </p>
    )
  }

  const choices = orderedChoices(request.options)
  const primary = choices.find((choice) => choice === 'allow_once' || choice === 'allow_session')
  const mark = MARK[request.provider as AiProvider]

  return (
    <section className="approval-card" role="group" aria-label={W.approvalCard.aria1} aria-busy={item.status === 'sending'}>
      <div className="approval-head">
        {mark && <ProviderMark provider={mark} className="approval-mark" />}
        <strong>{headline(request, title, change, targets.length, place)}</strong>
      </div>
      {request.kind === 'command' ? (
        <pre className="approval-command">{command}</pre>
      ) : targets.length > 1 ? (
        <ul className="approval-targets">
          {targets.map((target, index) => (
            <li key={`${target.path}-${index}`} title={target.path}>
              <span className="approval-target-name">{fileName(target.path)}</span>
              {target.change && <span className="approval-target-change">{W.approvalCard.targetChange[target.change]}</span>}
            </li>
          ))}
        </ul>
      ) : request.kind === 'file_change' ? (
        subject && <p className="approval-subject" title={path}>{subject}</p>
      ) : (
        request.description.trim() && <p className="approval-subject is-plain">{request.description}</p>
      )}
      {request.reason && <p className="approval-reason">{request.reason}</p>}
      {item.error && <p className="approval-error" role="alert">{item.error}</p>}
      <div className="approval-actions">
        {choices.map((choice) => (
          <button
            key={choice}
            className={choice === primary ? 'approval-button is-primary' : 'approval-button'}
            type="button"
            disabled={item.status === 'sending'}
            onClick={() => onAnswer(choice)}
          >
            {CHOICE_LABEL[choice]}
          </button>
        ))}
      </div>
    </section>
  )
}
