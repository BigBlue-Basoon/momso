import { W } from '../words'
import { useEffect, useState } from 'react'
import { Check } from 'lucide-react'
import { findZetId, readZet } from '../archive'
import { PROVIDER_NAME, type AiProvider } from '../ai-models'
import { CHOICE_DONE, CHOICE_LABEL, approvalChange, isBrowserApproval, orderedChoices, type ApprovalChange, type ApprovalChoice, type ApprovalRequest, type SetupAction } from '../approvals'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import './ApprovalCard.css'

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }

export type ApprovalItem = {
  request: ApprovalRequest
  /** 물음이 온 대화. */
  chatId: string | null
  /** 물음이 왔을 때 그 대화의 말 수. 답한 뒤 남는 한 줄을 그 자리에 둔다. */
  at: number
  /** closed: 답하기 전에 뒤쪽이 물음을 닫음(agent-approval-closed). 답한 것처럼 그 자리에 한 줄로 남는다. */
  status: 'pending' | 'sending' | 'answered' | 'closed'
  /** 닫힌 까닭(timeout·provider_cancelled, 옛 값이면 null). */
  closedReason?: string | null
  /** 선생님이 이 채팅방의 답을 멈췄다. 그 뒤 AI 쪽이 거두어 닫히면 줄 없이 거둔다("답을 중간에 멈췄습니다"가 이미 뜬다). */
  stopping?: boolean
  choice?: ApprovalChoice
  error?: string | null
}

/** 답하기 전에 닫힌 물음의 한 줄(답한 줄과 같은 꼴, 확인 표시 없이). 까닭 값이 없으면(옛 값) "닫혔습니다"까지만. */
export function closedText(reason: string | null | undefined, plain: string, timeout: string) {
  return reason === 'timeout' ? timeout : reason === 'provider_cancelled' ? W.approvals.closedByAi : plain
}
function ClosedLine({ subject, reason }: { subject: string | null; reason?: string | null }) {
  return (
    <p className="approval-done">
      <span>{closedText(reason, W.approvals.closed, W.approvals.closedTimeout)}</span>
      {subject && <span className="approval-done-subject">{subject}</span>}
    </p>
  )
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

/**
 * 코덱스가 보호 장치(샌드박스) 밖에서 명령을 다시 실행하려고 묻는지(2026-11-18 실제 앱 점검 B67).
 * 뒤쪽(chat_codex.rs)은 이것을 따로 알리는 값 없이 코덱스의 까닭 글(reason, 예: "command failed; retry without sandbox?")만 넘긴다.
 * 그래서 지금은 그 글로 가린다. 뒤쪽이 값을 주면 그것으로 바꾼다.
 */
function isOutsideSandbox(request: ApprovalRequest) {
  return request.provider === 'codex' && request.kind === 'command' && /sandbox/i.test(request.reason ?? '')
}

function headline(request: ApprovalRequest, title: string | null, change: ApprovalChange | null, count: number, place: 'file' | 'folder' | 'none') {
  const name = providerName(request.provider)
  if (isOutsideSandbox(request)) return W.approvalCard.outsideSandbox(name)
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

  if (item.status === 'closed') return <ClosedLine subject={subject} reason={item.closedReason} />
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
 * 몸소 브라우저의 물음(browser.ts). 머리 글, 하려는 일(뒤쪽이 적어 준 글 그대로: 스킬·작업·버튼 이름, 입력이면 입력할 값),
 * 사이트, 누르면 생기는 일(효과 값을 쉬운 말로, 모르는 값이면 적지 않음). 읽기만에서 바꾸는 일로 넘어가는 물음에는
 * "허락해도 누를 때마다 다시 묻는다"는 한 줄. 단추는 "이번만 허락"과 "거절"(orderedChoices).
 * reason 은 AI 의 까닭 글이 아니라 값이라(scope, publish …) 까닭 줄로 보이지 않는다.
 */
function BrowserApprovalCard({ item, onAnswer }: { item: ApprovalItem; onAnswer: (choice: ApprovalChoice) => void }) {
  const { request } = item
  const words = W.approvalCard.browser
  const who = providerName(request.provider)
  const scope = request.kind === 'browser_action_scope'
  const site = request.targets?.[0]?.path?.trim() || null
  const effect = !scope && request.reason ? (words.effect as Record<string, string>)[request.reason] ?? null : null
  const subject = request.description.trim()

  if (item.status === 'closed') return <ClosedLine subject={subject || null} reason={item.closedReason} />
  if (item.status === 'answered' && item.choice) {
    return (
      <p className="approval-done">
        <Check size={13} aria-hidden />
        <span>{CHOICE_DONE[item.choice]}</span>
        {subject && <span className="approval-done-subject">{subject}</span>}
      </p>
    )
  }

  const choices = orderedChoices(request.options, request)
  const mark = MARK[request.provider as AiProvider]
  return (
    <section className="approval-card is-setup is-browser" role="group" aria-label={W.approvalCard.aria1} aria-busy={item.status === 'sending'}>
      <div className="approval-head">
        {mark && <ProviderMark provider={mark} className="approval-mark" />}
        <strong>{scope ? words.scopeHead(who) : words.actionHead(who)}</strong>
      </div>
      {subject && <p className="approval-subject is-plain">{subject}</p>}
      {(site || effect) && (
        <dl className="approval-facts">
          {site && <div><dt>{words.site}</dt><dd>{site}</dd></div>}
          {effect && <div><dt>{words.result}</dt><dd>{effect}</dd></div>}
        </dl>
      )}
      {scope && <p className="approval-reason">{words.scopeNote}</p>}
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
  if (isBrowserApproval(request)) return <BrowserApprovalCard item={item} onAnswer={onAnswer} />

  if (item.status === 'closed') return <ClosedLine subject={subject} reason={item.closedReason} />
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
  const outside = isOutsideSandbox(request)
  // 보호 장치 밖 실행은 허락 단추를 앞세우지 않는다(위험한 쪽을 기본으로 두지 않음).
  const primary = outside ? undefined : choices.find((choice) => choice === 'allow_once' || choice === 'allow_session')
  const mark = MARK[request.provider as AiProvider]

  return (
    <section className={outside ? 'approval-card is-danger' : 'approval-card'} role="group" aria-label={W.approvalCard.aria1} aria-busy={item.status === 'sending'}>
      <div className="approval-head">
        {mark && <ProviderMark provider={mark} className="approval-mark" />}
        <strong>{headline(request, title, change, targets.length, place)}</strong>
      </div>
      {outside ? (
        <>
          <p className="approval-danger" role="alert">{W.approvalCard.outsideSandboxWarn}</p>
          {/* 명령과 코덱스가 보낸 글(영어)은 접어 둔다. 펼치면 받은 그대로. */}
          <details className="approval-more">
            <summary>{W.approvalCard.showCommand}</summary>
            <pre className="approval-command">{command}</pre>
            {request.reason && <p className="approval-reason">{request.reason}</p>}
          </details>
        </>
      ) : request.kind === 'command' ? (
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
      {request.reason && !outside && <p className="approval-reason">{request.reason}</p>}
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
