import { Check, Copy, Wand2 } from 'lucide-react'
import { ShortcutTip } from './ShortcutTip'
import { W } from '../words'
import { Fragment, type ReactNode, useEffect, useRef, useState } from 'react'
import { PROVIDER_NAME, type AiProvider } from '../ai-models'
import type { ApprovalChoice } from '../approvals'
import { ApprovalCard, type ApprovalItem } from './ApprovalCard'
import { SecretRequestCard, type SecretItem } from './SecretRequestCard'
import { MessageRail } from './MessageRail'
import { ZetMarkdown } from './ZetMarkdown'
import { openWebAddress } from '../archive'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import './ChatColumn.css'

export type ChatViewMessage = {
  id: string
  role: 'user' | 'assistant'
  text: string
  /** AI 의 답이면 답한 곳과 모델 이름(알 때만). */
  provider?: AiProvider | null
  modelLabel?: string | null
  /** 곳을 바꿔 보냈는데 앞의 말을 넘기지 못하고 쓴 답이면 넘기지 못한 말의 수(그 답 위에 알림 한 줄). 0 이면 없음. */
  notHandedOver?: number
  /** 내 말에 고른 스킬의 이름(한글 이름, 모르면 부르는 이름). */
  skill?: string | null
  /** 끊긴 답이면 그 까닭(chat_read 의 finishReason, completed 가 아닌 것). 정상으로 끝났거나 옛 말이면 null. */
  endReason?: string | null
  /** 말을 보낸·받은 때(chat_read 의 createdAt). 말 아래 복사 단추 옆에 보인다. 없으면 적지 않는다. */
  createdAt?: string | null
}

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }

// 답의 모습. 'a'(기본): 글씨는 세 곳이 같고 답 머리에 회사 로고. 'b': a + 회사마다 아주 조금 다른 글자 색.
// 개발용 실행에서만 localStorage 'momso.dev.answerStyle' 에 'b' 를 넣고 새로 고치면 b 로 볼 수 있다. 내보내는 빌드는 늘 'a'.
const ANSWER_STYLE: 'a' | 'b' = (() => {
  if (!import.meta.env.DEV) return 'a'
  try {
    return window.localStorage.getItem('momso.dev.answerStyle') === 'b' ? 'b' : 'a'
  } catch {
    return 'a'
  }
})()

type AssistantPanelProps = {
  /** 대화 칸 맨 위 줄의 오른쪽 끝에 놓는 단추. */
  headerActions?: ReactNode
  /** 지금 대화의 이름. 아직 없으면 빈 글자. */
  title: string
  /** 이름을 바꿀 수 있는지. 새 대화를 쓰는 중(아직 저장 전)이면 바꿀 수 없다. */
  canRename: boolean
  messages: ChatViewMessage[]
  isGenerating: boolean
  /** 답이 오는 중이면 지금까지 온 글. */
  streamText: string
  onRename: (title: string) => void
  /** 아래의 입력 칸(Composer). */
  composer: ReactNode
  /** 이 대화에서 AI 가 허락을 구한 물음들(답한 것은 한 줄로 남는다). */
  approvals: ApprovalItem[]
  onAnswerApproval: (requestId: string, choice: ApprovalChoice) => void
  /** 답 안의 문서 링크를 누르면 그 문서를 연다. */
  onOpenDocument: (id: string) => void
  /** 채팅방 안의 알림 한 줄(앞의 내용을 줄여 담음 등). at 은 그때까지의 말 수. */
  notices?: Array<{ id: string; at: number; text: string }>
  /** 스킬로 하는 연결에서 키를 넣어 달라는 물음(끝난 것은 한 줄로 남는다). */
  secretRequests?: SecretItem[]
  onSubmitSecret?: (requestId: string, values: Record<string, string>) => void
  /** 키 넣기를 그만두기(agent_setup_secret_cancel). 넘기지 않으면 "그만두기" 단추를 두지 않는다. */
  onCancelSecret?: (requestId: string) => void
  /** 답이 오지 않았을 때 채팅방 끝의 한 줄과 지금 할 수 있는 일(G18). 다음에 보내면 사라진다. */
  failure?: { text: string; actions: Array<{ id: string; label: string; run: () => void }> } | null
}

/**
 * 가운데 대화 칸. 맨 위 줄 왼쪽에 대화 이름(누르면 이름 바꾸기), 오른쪽 끝에 패널 단추,
 * 왼쪽 가장자리에 막대 목차, 아래에 입력 칸.
 */
export function AssistantPanel({
  headerActions,
  title,
  canRename,
  messages,
  isGenerating,
  streamText,
  onRename,
  composer,
  approvals,
  onAnswerApproval,
  onOpenDocument,
  notices = [],
  secretRequests = [],
  onSubmitSecret,
  onCancelSecret,
  failure = null,
}: AssistantPanelProps) {
  // 답은 보는 창과 같은 도구로 그린다(목록, 굵은 글씨, 제목, 코드, 표, 링크). 링크는 앱의 여는 방식(opener)으로.
  const openAddress = (address: string) => { void openWebAddress(address).catch(() => {}) }
  const panelRef = useRef<HTMLElement>(null)
  const streamRef = useRef<HTMLDivElement>(null)
  const userMessageCount = messages.filter((message) => message.role === 'user').length
  // 마지막 답의 복사 단추와 때는 늘 보이고(Aside 앱 사진처럼), 나머지 말은 올렸을 때만 보인다.
  const lastAssistantIndex = messages.map((message) => message.role).lastIndexOf('assistant')
  const shownTitle = title.trim() || W.assistantPanel.text1

  // 답한 물음은 물음이 왔던 자리(그때까지의 말 뒤)에 한 줄로 남고, 답을 기다리는 물음은 대화 끝에 뜬다.
  const answeredAt = (index: number) => approvals.filter((item) => item.status === 'answered' && Math.min(item.at, messages.length) === index)
  const pendingApprovals = approvals.filter((item) => item.status !== 'answered')
  const noticesAt = (index: number) => notices.filter((item) => Math.min(item.at, messages.length) === index)
  const renderNotice = (item: { id: string; text: string }) => <p key={item.id} className="message-notice" role="note">{item.text}</p>
  const renderApproval = (item: ApprovalItem) => (
    <ApprovalCard key={item.request.requestId} item={item} onAnswer={(choice) => onAnswerApproval(item.request.requestId, choice)} />
  )
  // 키 넣기: 끝난 것은 물음이 온 자리에 한 줄로, 기다리는 것은 대화 끝에.
  const secretsDoneAt = (index: number) => secretRequests.filter((item) => (item.status === 'saved' || item.status === 'cancelled') && Math.min(item.at, messages.length) === index)
  const pendingSecrets = secretRequests.filter((item) => item.status === 'pending' || item.status === 'sending')
  const renderSecret = (item: SecretItem) => (
    <SecretRequestCard
      key={item.request.requestId}
      item={item}
      onSubmit={(values) => onSubmitSecret?.(item.request.requestId, values)}
      onCancel={onCancelSecret ? () => onCancelSecret(item.request.requestId) : undefined}
    />
  )

  // 새 말이나 물음이 붙으면 맨 아래로 내린다.
  useEffect(() => {
    const stream = streamRef.current
    if (stream) stream.scrollTo({ top: stream.scrollHeight, behavior: 'smooth' })
  }, [messages.length, isGenerating, pendingApprovals.length, pendingSecrets.length])

  // 입력 칸이 자라면 대화의 마지막 말이 그 뒤에 가리지 않게 아래 여백을 함께 늘린다.
  useEffect(() => {
    const panel = panelRef.current
    const dock = panel?.querySelector<HTMLElement>('.composer-dock')
    if (!panel || !dock) return
    const observer = new ResizeObserver(() => panel.style.setProperty('--composer-space', `${dock.offsetHeight}px`))
    observer.observe(dock)
    return () => observer.disconnect()
  }, [])

  return (
    <section ref={panelRef} className="assistant-panel" aria-label={W.assistantPanel.aria1} data-answer-style={ANSWER_STYLE}>
      <header className="assistant-header">
        <ChatTitle key={shownTitle} title={shownTitle} canRename={canRename} onRename={onRename} />
        {headerActions && <div className="assistant-header-actions">{headerActions}</div>}
      </header>

      <MessageRail streamRef={streamRef} userMessageCount={userMessageCount} />

      <div ref={streamRef} className="message-stream" aria-live="polite">
        {/* 아직 말이 없는 채팅방(홈): 한가운데에 몸소 CI 와 한 줄(2026-11-16 그의 말, Codex 앱의 빈 화면처럼). 첫 말을 보내면 사라진다.
            CI 는 docs/brand 의 메인 로고 agar yoga(인앱 로고) 심볼만, Codex 앱처럼 무채색으로(2026-11-16 그의 말 "심볼 넣을 떄 무채색으로").
            public/brand/agar-yoga-mono-mask.png 는 원본(agar-yoga.png = agar-yoga-mark-transparent.png)에서 만든 한 색 가림판 사본이다. */}
        {messages.length === 0 && !isGenerating && pendingApprovals.length === 0 && pendingSecrets.length === 0 && notices.length === 0 && (
          <div className="chat-empty-hero">
            <span className="chat-empty-mark" aria-hidden="true" />
            <p className="chat-empty-title">{W.assistantPanel.emptyTitle}</p>
          </div>
        )}
        {messages.map((message, index) => (
          <Fragment key={message.id}>
            {answeredAt(index).map(renderApproval)}
            {secretsDoneAt(index).map(renderSecret)}
            {noticesAt(index).map(renderNotice)}
            {Boolean(message.notHandedOver) && message.provider && (
              <p className="message-notice" role="note">{W.assistantPanel.text2(PROVIDER_NAME[message.provider], message.notHandedOver ?? 0)}</p>
            )}
            {message.role === 'user' ? (
              // 내 말: 이름표 없이 말풍선만(두 앱처럼 오른쪽에).
              <div className="message user">
                {message.skill && (
                  <span className="message-skill" aria-label={W.skills.usedLabel(message.skill)}>
                    <Wand2 size={12} aria-hidden />{message.skill}
                  </span>
                )}
                {message.text && <p>{message.text}</p>}
                {message.text && <MessageActions text={message.text} at={message.createdAt} side="user" />}
              </div>
            ) : (
              // AI 의 답: 말풍선 없이 바탕 위에 글. 답한 회사를 알면 머리에 그 회사 로고를 작게(올리면 모델 이름).
              <div className="message assistant" data-provider={message.provider ?? undefined}>
                {message.provider && (
                  <span className="message-by" title={answeredBy(message.provider, message.modelLabel)}>
                    <ProviderMark provider={MARK[message.provider]} className="message-by-mark" />
                    <span className="sr-only">{answeredBy(message.provider, message.modelLabel)}</span>
                  </span>
                )}
                <ZetMarkdown text={message.text} onOpenZet={onOpenDocument} onOpenAddress={openAddress} />
                {message.text && (
                  <MessageActions text={message.text} at={message.createdAt} side="assistant" always={index === lastAssistantIndex && !isGenerating} />
                )}
              </div>
            )}
            {/* 끊긴 답에만 그 아래 한 줄. 까닭은 답에 저장돼 앱을 다시 켜도 남는다. 모르는 값이면 적지 않는다. */}
            {/* 답이 오지 않은 까닭 줄(failure)이 보이면 마지막 답의 "문제가 생겨 끊겼습니다"는 같은 말이라 두지 않는다. */}
            {message.role === 'assistant' && message.endReason && W.assistantPanel.endReason[message.endReason]
              && !(failure && !isGenerating && index === lastAssistantIndex && message.endReason === 'error') && (
              <p className="message-notice message-end" role="note">{W.assistantPanel.endReason[message.endReason]}</p>
            )}
          </Fragment>
        ))}
        {answeredAt(messages.length).map(renderApproval)}
        {secretsDoneAt(messages.length).map(renderSecret)}
        {noticesAt(messages.length).map(renderNotice)}
        {isGenerating && (
          <div className="message assistant generating">
            {streamText ? (
              // 흘러 들어오는 중에도 같은 도구로 그린다. 닫히지 않은 굵은 글씨·코드 조각은 닫힐 때까지 글자 그대로 둔다.
              <ZetMarkdown text={streamText} onOpenZet={onOpenDocument} onOpenAddress={openAddress} />
            ) : (
              // 두 앱처럼 흐린 글 한 줄(도는 그림 없음).
              <span className="thinking">{W.assistantPanel.text3}</span>
            )}
          </div>
        )}
        {/* 답이 오지 않은 까닭 한 줄(있던 알림 줄 꼴)과 그 자리에서 할 수 있는 일(카드의 글자 단추 꼴). */}
        {failure && !isGenerating && (
          <div className="message-notice message-failure" role="alert">
            <span>{failure.text}</span>
            {failure.actions.length > 0 && (
              <span className="message-failure-actions">
                {failure.actions.map((action) => (
                  <button key={action.id} className="approval-button" type="button" onClick={action.run}>{action.label}</button>
                ))}
              </span>
            )}
          </div>
        )}
        {pendingApprovals.map(renderApproval)}
        {pendingSecrets.map(renderSecret)}
      </div>

      {composer}
    </section>
  )
}

const timeOfDay = new Intl.DateTimeFormat('ko-KR', { hour: 'numeric', minute: '2-digit' })
const monthDay = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric' })
const yearMonthDay = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
const relativeDay = new Intl.RelativeTimeFormat('ko-KR', { numeric: 'auto' })

/** 말의 때(Aside 앱처럼): 오늘이면 "오후 2:40", 어제면 "어제 오후 2:40", 올해면 "11월 14일 오후 2:40", 그 전이면 해까지. */
function messageTime(at?: string | null) {
  if (!at) return null
  const date = new Date(at)
  if (Number.isNaN(date.getTime())) return null
  const now = new Date()
  const days = Math.round((new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() - new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()) / 86_400_000)
  const time = timeOfDay.format(date)
  if (days <= 0) return time
  if (days === 1) return `${relativeDay.format(-1, 'day')} ${time}`
  return `${(date.getFullYear() === now.getFullYear() ? monthDay : yearMonthDay).format(date)} ${time}`
}

/** 글을 붙여 넣을 수 있게 담는다. 클립보드 명령이 막히면 숨긴 글 칸으로 한 번 더 해 본다. */
async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    const area = document.createElement('textarea')
    area.value = text
    area.setAttribute('readonly', '')
    area.style.position = 'fixed'
    area.style.opacity = '0'
    document.body.appendChild(area)
    area.select()
    const done = document.execCommand('copy')
    area.remove()
    return done
  }
}

/**
 * 말 아래의 복사 단추와 때(Aside·Codex 앱처럼). 내 말은 말풍선 아래 오른쪽에 "때 · 복사", AI 의 답은 답 아래 왼쪽에
 * "복사 · 때". 마지막 답은 늘 보이고 나머지는 말에 올리거나 키보드로 가면 보인다. 누르면 그 말의 글 그대로
 * (답은 마크다운 원문) 담기고, 그림이 잠깐 체크로 바뀌며 풍선이 "복사했습니다"가 된다.
 * Aside 의 복사 옆 단추(Branch, 그 말에서 새 채팅으로 가지치기)는 몸소에 없는 기능이라 두지 않는다.
 */
function MessageActions({ text, at, side, always = false }: { text: string; at?: string | null; side: 'user' | 'assistant'; always?: boolean }) {
  const [copied, setCopied] = useState(false)
  const timer = useRef<number | null>(null)
  useEffect(() => () => { if (timer.current) window.clearTimeout(timer.current) }, [])
  const label = copied ? W.messageActions.copied : W.messageActions.copy
  const time = messageTime(at)
  const copy = async () => {
    if (!(await copyText(text))) return
    setCopied(true)
    if (timer.current) window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setCopied(false), 1500)
  }
  const button = (
    <ShortcutTip label={label}>
      <button className="message-action" type="button" aria-label={label} onClick={() => void copy()}>
        {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />}
      </button>
    </ShortcutTip>
  )
  const stamp = time && <time className="message-time" dateTime={at ?? undefined}>{time}</time>
  return (
    <div className={`message-actions is-${side}${always ? ' is-always' : ''}`}>
      {side === 'user' ? <>{stamp}{button}</> : <>{button}{stamp}</>}
    </div>
  )
}

/** 답한 곳과 모델. 모델 이름에 곳 이름이 이미 들어 있으면 한 번만 쓴다. */
function answeredBy(provider: AiProvider, modelLabel?: string | null) {
  const name = PROVIDER_NAME[provider]
  if (!modelLabel) return name
  return modelLabel.toLowerCase().includes(name.toLowerCase()) ? modelLabel : `${name} ${modelLabel}`
}

/** 맨 위 줄의 대화 이름. 누르면 그 자리에서 이름을 바꾼다(Enter 로 바꾸기, Esc 로 그만두기). */
function ChatTitle({ title, canRename, onRename }: { title: string; canRename: boolean; onRename: (title: string) => void }) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(title)
  const input = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (editing) input.current?.select()
  }, [editing])

  if (!canRename) return <h1 className="chat-title">{title}</h1>

  if (editing) {
    const finish = (save: boolean) => {
      setEditing(false)
      const next = draft.trim()
      if (save && next && next !== title) onRename(next)
    }
    return (
      <input
        ref={input}
        className="chat-title-input"
        value={draft}
        aria-label={W.assistantPanel.aria2}
        maxLength={80}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={() => finish(true)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && !event.nativeEvent.isComposing) finish(true)
          if (event.key === 'Escape') finish(false)
        }}
      />
    )
  }

  return (
    <button
      className="chat-title chat-title-button"
      type="button"
      title={W.assistantPanel.tip1}
      onClick={() => {
        setDraft(title)
        setEditing(true)
      }}
    >
      {/* 이름이 바뀌면(백엔드가 짧은 이름을 지어 붙임) 새 이름이 부드럽게 나타난다. */}
      <span key={title} className="chat-title-text">{title}</span>
    </button>
  )
}
