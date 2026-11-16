import { W } from '../words'
import { useEffect, useState } from 'react'
import { Check } from 'lucide-react'
import { findZetId, readZet } from '../archive'
import { PROVIDER_NAME, type AiProvider } from '../ai-models'
import { CHOICE_DONE, CHOICE_LABEL, approvalChange, orderedChoices, type ApprovalChange, type ApprovalChoice, type ApprovalRequest, type SetupAction } from '../approvals'
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

/** 회사 이름: GitHub 은 그대로, 원자료 저장소 회사는 설정과 같은 이름(AWS·GCP·Cloudflare·NCP). 모르는 값은 받은 그대로. */
export function serviceName(service: string) {
  if (service === 'github') return 'GitHub'
  return (W.backup.storeName as Record<string, string>)[service] ?? service
}

/**
 * 스킬로 하는 연결에서 AI 가 무언가를 만들고 연결하려 할 때의 물음(kind 'setup_change', approvals.ts 의 계약).
 * 파일 대신 "무엇을 하려는지": 어느 회사의 어느 계정에, 지역, 새로 만들 것의 정확한 이름, 보이는 사람, 돈이 들 수 있는지(온 것만).
 * 한 번 한 번 묻는 일이라 "이번만 허락"과 "거절"만 둔다(orderedChoices).
 */
function SetupApprovalCard({ item, setup, onAnswer }: { item: ApprovalItem; setup: SetupAction; onAnswer: (choice: ApprovalChoice) => void }) {
  const { request } = item
  const who = providerName(request.provider)
  const where = serviceName(setup.service)
  const words = W.approvalCard.setup
  const known = setup.action === 'create_and_connect'
  // GitHub 은 저장소, 원자료 저장소 회사(Cloudflare·GCP·AWS)는 원자료를 보관할 곳.
  const head = !known ? W.approvalCard.text4(who)
    : setup.service === 'github' ? words.createRepository(who, where) : words.createStorage(who, where)
  // 끝난 뒤 남는 한 줄: 같은 이름을 여러 회사에 만들 수 있어 회사 이름을 앞에 붙인다(예: GitHub momso-teacher).
  const subject = setup.resourceName ? `${where} ${setup.resourceName}` : where
  const visibility = setup.visibility ? words.visibilityText[setup.visibility] : null
  const cost = setup.cost ? words.costText[setup.cost] : null

  if (item.status === 'answered' && item.choice) {
    return (
      <p className="approval-done">
        <Check size={13} aria-hidden />
        <span>{CHOICE_DONE[item.choice]}</span>
        <span className="approval-done-subject">{subject}</span>
      </p>
    )
  }

  const choices = orderedChoices(request.options, request)
  const mark = MARK[request.provider as AiProvider]
  return (
    <section className="approval-card is-setup" role="group" aria-label={W.approvalCard.aria1} aria-busy={item.status === 'sending'}>
      <div className="approval-head">
        {mark && <ProviderMark provider={mark} className="approval-mark" />}
        <strong>{head}</strong>
      </div>
      <dl className="approval-facts">
        {/* 회사 이름은 머리 글에 있으니 다시 적지 않는다. 머리 글을 모를 때(그 밖의 일)만 적는다. */}
        {!known && <div><dt>{words.service}</dt><dd>{where}</dd></div>}
        {setup.account && <div><dt>{words.account}</dt><dd>{setup.account}</dd></div>}
        {setup.region && <div><dt>{words.region}</dt><dd>{words.regionName[setup.region] ?? setup.region}</dd></div>}
        {setup.resourceName && <div><dt>{words.name}</dt><dd className="approval-fact-name">{setup.resourceName}</dd></div>}
        {visibility && <div><dt>{words.visibility}</dt><dd>{visibility}</dd></div>}
        {cost && <div><dt>{words.cost}</dt><dd>{cost}</dd></div>}
      </dl>
      {!known && request.description.trim() && (
        <p className="approval-subject is-plain">{request.description}</p>
      )}
      {request.reason && <p className="approval-reason">{request.reason}</p>}
      {item.error && <p className="approval-error" role="alert">{item.error}</p>}
      <div className="approval-actions">
        {choices.map((choice) => (
          <button
            key={choice}
            className={choice === 'allow_once' ? 'approval-button is-primary' : 'approval-button'}
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

  if (request.kind === 'setup_change' && request.setup) return <SetupApprovalCard item={item} setup={request.setup} onAnswer={onAnswer} />

  if (item.status === 'answered' && item.choice) {
    return (
      <p className="approval-done">
        <Check size={13} aria-hidden />
        <span>{CHOICE_DONE[item.choice]}</span>
        <span className="approval-done-subject">{subject}</span>
      </p>
    )
  }

  const choices = orderedChoices(request.options, request)
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
