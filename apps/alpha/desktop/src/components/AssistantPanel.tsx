import { AudioLines, Check, Copy, FileText, Film, Globe, Image as ImageIcon, Wand2 } from 'lucide-react'
import type { AttachedFile } from '../chat-items'
import { ShortcutTip } from './ShortcutTip'
import { W } from '../words'
import { Fragment, memo, type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { PROVIDER_NAME, type AiProvider } from '../ai-models'
import type { ApprovalChoice } from '../approvals'
import { ApprovalCard, type ApprovalItem } from './ApprovalCard'
import { SecretRequestCard, type SecretItem } from './SecretRequestCard'
import { SavedLoginChoiceCard, type LoginChoiceItem } from './SavedLoginChoiceCard'
import { MessageRail } from './MessageRail'
import { ZetMarkdown } from './ZetMarkdown'
import { requestPanelBrowser } from '../panel-browser'
import { observeSize } from '../resize-later'
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
  /** 내 말에 붙여 보낸 첨부(파일 이름과 종류). */
  attachments?: AttachedFile[]
  /** 끊긴 답이면 그 까닭(chat_read 의 finishReason, completed 가 아닌 것). 정상으로 끝났거나 옛 말이면 null. */
  endReason?: string | null
  /** 말을 보낸·받은 때(chat_read 의 createdAt). 말 아래 복사 단추 옆에 보인다. 없으면 적지 않는다. */
  createdAt?: string | null
}

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }
/**
 * 말이 많은 채팅방을 열 때 처음에 그리는 말의 수와, 그 위의 옛 말을 이어서 그리는 한 번의 수(2026-11-17 많이 쌓였을 때 재기:
 * 말 500개 채팅방을 여는 데 1.4초). 다 그려진 뒤의 모습은 같다. 처음 보이는 것은 늘 맨 아래 말들이다.
 */
const FIRST_WINDOW = 60
const FILL_STEP = 40

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
  /** 대화 칸 맨 위 줄의 왼쪽 끝에 놓는 단추(네트워크 길의 휴대폰 너비에서 채팅방 목록 열기). */
  headerStart?: ReactNode
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
  /** 이 채팅방의 몸소 브라우저가 열려 있는지(browser-session-updated, browser_session_status). */
  browserOpen?: boolean
  /** 스킬로 하는 연결에서 키를 넣어 달라는 물음(끝난 것은 한 줄로 남는다). */
  secretRequests?: SecretItem[]
  onSubmitSecret?: (requestId: string, values: Record<string, string>) => void
  /** 키 넣기를 그만두기(agent_setup_secret_cancel). 넘기지 않으면 "그만두기" 단추를 두지 않는다. */
  onCancelSecret?: (requestId: string) => void
  /** 저장된 로그인 중 어느 것을 쓸지 고르는 물음(saved-login-selection-request). 고른 것은 한 줄로 남는다. */
  loginChoices?: LoginChoiceItem[]
  onChooseLogin?: (requestId: string, loginId: string) => void
  onCancelLogin?: (requestId: string) => void
  /** 답이 오지 않았을 때 채팅방 끝의 한 줄과 지금 할 수 있는 일(G18). 다음에 보내면 사라진다. */
  failure?: { text: string; actions: Array<{ id: string; label: string; run: () => void }> } | null
  /** 회사가 스스로 다시 시도하는 중일 때 답 아래 조용한 한 줄(chat-provider-retrying). */
  retrying?: string | null
  /** 지금 채팅방(바뀌면 맨 아래부터 다시 그린다). 긴 채팅방 페이지(chat_read_page): 더 옛 말이 있는지, 불러오는 중인지, 불러오기. */
  chatKey?: string | null
  hasOlder?: boolean
  loadingOlder?: boolean
  onLoadOlder?: () => void
}

/**
 * 가운데 대화 칸. 맨 위 줄 왼쪽에 대화 이름(누르면 이름 바꾸기), 오른쪽 끝에 패널 단추,
 * 왼쪽 가장자리에 막대 목차, 아래에 입력 칸.
 */
export function AssistantPanel({
  headerActions,
  headerStart,
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
  browserOpen = false,
  secretRequests = [],
  onSubmitSecret,
  onCancelSecret,
  loginChoices = [],
  onChooseLogin,
  onCancelLogin,
  failure = null,
  retrying = null,
  chatKey = null,
  hasOlder = false,
  loadingOlder = false,
  onLoadOlder,
}: AssistantPanelProps) {
  // 답은 보는 창과 같은 도구로 그린다(목록, 굵은 글씨, 제목, 코드, 표, 링크). 링크는 앱의 여는 방식(opener)으로.
  // 두 손잡이는 늘 같은 함수로 넘긴다: 입력창에 글자를 칠 때나 답이 흘러 들어올 때 지난 말들을 다시 그리지 않게(2026-11-17 많이 쌓였을 때 재기).
  // 답 안의 바깥 링크는 오른쪽 패널의 브라우저 탭에서 연다(2026-11-17 본부). 패널이 닫혀 있으면 App 이 연다.
  const openAddress = useCallback((address: string) => { requestPanelBrowser(address) }, [])
  const openDocumentRef = useRef(onOpenDocument)
  openDocumentRef.current = onOpenDocument
  const openDocument = useCallback((id: string) => openDocumentRef.current(id), [])
  const panelRef = useRef<HTMLElement>(null)
  const streamRef = useRef<HTMLDivElement>(null)
  const userMessageCount = messages.filter((message) => message.role === 'user').length
  // 마지막 답의 복사 단추와 때는 늘 보이고(Aside 앱 사진처럼), 나머지 말은 올렸을 때만 보인다.
  const lastAssistantIndex = messages.map((message) => message.role).lastIndexOf('assistant')
  const shownTitle = title.trim() || W.assistantPanel.text1

  // 채팅방이 바뀌면 맨 아래 FIRST_WINDOW 개만 그린다. 옛 말은 위로 올려 읽을 때만 FILL_STEP 개씩 위에 더하고(2026-11-18: 예전에는
  // 쉬는 틈마다 다 그려 2,000개 채팅방에서 말풍선이 모두 그려졌다), 불러온 말을 다 그렸으면 이전 페이지를 부른다(onLoadOlder).
  // 같은 채팅방에 옛 말이 앞에 붙으면(이전 페이지) 그린 범위를 그만큼 밀어 보던 말이 그대로 있게 한다.
  const firstId = messages[0]?.id ?? ''
  const [windowStart, setWindowStart] = useState(() => ({ key: chatKey, firstId, from: Math.max(0, messages.length - FIRST_WINDOW) }))
  if (windowStart.key !== chatKey) setWindowStart({ key: chatKey, firstId, from: Math.max(0, messages.length - FIRST_WINDOW) })
  else if (windowStart.firstId !== firstId) {
    const added = messages.findIndex((message) => message.id === windowStart.firstId)
    setWindowStart({ key: chatKey, firstId, from: added >= 0 ? windowStart.from + added : Math.max(0, messages.length - FIRST_WINDOW) })
  }
  const shownFrom = windowStart.key === chatKey && windowStart.firstId === firstId ? Math.min(windowStart.from, messages.length) : Math.max(0, messages.length - FIRST_WINDOW)
  const olderRef = useRef({ shownFrom, hasOlder, loadingOlder, onLoadOlder })
  olderRef.current = { shownFrom, hasOlder, loadingOlder, onLoadOlder }
  const onStreamScroll = useCallback(() => {
    const stream = streamRef.current
    // 사람이 위로 올려 읽을 때만(휠·키·막대 끌기). 처음 열 때 맨 위에서 맨 아래로 내려가는 사이의 스크롤은 치지 않는다.
    if (!stream || !readingUp.current || stream.scrollTop > 400) return
    const now = olderRef.current
    if (now.shownFrom > 0) {
      readingUp.current = true
      fillAnchor.current = stream.scrollHeight - stream.scrollTop
      setWindowStart((current) => ({ ...current, from: Math.max(0, current.from - FILL_STEP) }))
    } else if (now.hasOlder && !now.loadingOlder) now.onLoadOlder?.()
  }, [])
  // 옛 말을 위에 더해도 보던 자리를 지킨다: 위로 올려 읽지 않았으면 맨 아래에, 올려 읽는 중이면 보던 말 그대로.
  const fillAnchor = useRef<number | null>(null)
  const readingUp = useRef(false)
  useEffect(() => { readingUp.current = false }, [chatKey])
  useLayoutEffect(() => {
    const stream = streamRef.current
    const anchor = fillAnchor.current
    fillAnchor.current = null
    if (!stream || anchor === null) return
    stream.scrollTop = readingUp.current ? stream.scrollHeight - anchor : stream.scrollHeight
  }, [shownFrom])

  // 답한 물음은 물음이 왔던 자리(그때까지의 말 뒤)에 한 줄로 남고, 답을 기다리는 물음은 대화 끝에 뜬다.
  // 답하기 전에 닫힌 물음(closed)도 답한 물음처럼 그 자리에 한 줄로 남는다.
  const answeredAt = (index: number) => approvals.filter((item) => (item.status === 'answered' || item.status === 'closed') && Math.min(item.at, messages.length) === index)
  const pendingApprovals = approvals.filter((item) => item.status !== 'answered' && item.status !== 'closed')
  const noticesAt = (index: number) => notices.filter((item) => Math.min(item.at, messages.length) === index)
  const renderNotice = (item: { id: string; text: string }) => <p key={item.id} className="message-notice" role="note">{item.text}</p>
  const renderApproval = (item: ApprovalItem) => (
    <ApprovalCard key={item.request.requestId} item={item} onAnswer={(choice) => onAnswerApproval(item.request.requestId, choice)} />
  )
  // 키 넣기: 끝난 것은 물음이 온 자리에 한 줄로, 기다리는 것은 대화 끝에.
  const secretsDoneAt = (index: number) => secretRequests.filter((item) => (item.status === 'saved' || item.status === 'cancelled' || item.status === 'closed') && Math.min(item.at, messages.length) === index)
  const pendingSecrets = secretRequests.filter((item) => item.status === 'pending' || item.status === 'sending')
  // 로그인 고르기: 키 넣기와 같이 끝난 것은 물음이 온 자리에, 기다리는 것은 대화 끝에.
  const choicesDoneAt = (index: number) => loginChoices.filter((item) => (item.status === 'chosen' || item.status === 'cancelled' || item.status === 'closed') && Math.min(item.at, messages.length) === index)
  const pendingChoices = loginChoices.filter((item) => item.status === 'pending' || item.status === 'sending')
  const renderChoice = (item: LoginChoiceItem) => (
    <SavedLoginChoiceCard
      key={item.request.requestId}
      item={item}
      onChoose={(loginId) => onChooseLogin?.(item.request.requestId, loginId)}
      onCancel={onCancelLogin && (() => onCancelLogin(item.request.requestId))}
    />
  )
  const renderSecret = (item: SecretItem) => (
    <SecretRequestCard
      key={item.request.requestId}
      item={item}
      onSubmit={(values) => onSubmitSecret?.(item.request.requestId, values)}
      onCancel={onCancelSecret ? () => onCancelSecret(item.request.requestId) : undefined}
    />
  )

  // 새 말이나 물음이 붙으면 맨 아래로 내린다(옛 말이 앞에 붙는 것은 새 말이 아니라서 마지막 말 id 로 본다).
  const lastId = messages[messages.length - 1]?.id ?? ''
  useEffect(() => {
    const stream = streamRef.current
    if (stream) stream.scrollTo({ top: stream.scrollHeight, behavior: 'smooth' })
  }, [lastId, isGenerating, pendingApprovals.length, pendingSecrets.length, pendingChoices.length])

  // 입력 칸이 자라면 대화의 마지막 말이 그 뒤에 가리지 않게 아래 여백을 함께 늘린다.
  useEffect(() => {
    const panel = panelRef.current
    const dock = panel?.querySelector<HTMLElement>('.composer-dock')
    if (!panel || !dock) return
    panel.style.setProperty('--composer-space', `${dock.offsetHeight}px`)
    return observeSize(dock, () => panel.style.setProperty('--composer-space', `${dock.offsetHeight}px`))
  }, [])

  return (
    <section ref={panelRef} className="assistant-panel" aria-label={W.assistantPanel.aria1} data-answer-style={ANSWER_STYLE}>
      <header className="assistant-header">
        {headerStart}
        <ChatTitle key={shownTitle} title={shownTitle} canRename={canRename} onRename={onRename} />
        {browserOpen && (
          <span className="assistant-browser" role="status">
            <Globe size={13} aria-hidden />{W.assistantPanel.browserOpen}
          </span>
        )}
        {headerActions && <div className="assistant-header-actions">{headerActions}</div>}
      </header>

      <MessageRail streamRef={streamRef} userMessageCount={userMessageCount} />

      <div
        ref={streamRef}
        className="message-stream"
        aria-live="polite"
        onScroll={onStreamScroll}
        onWheel={(event) => { if (event.deltaY < 0) readingUp.current = true }}
        onKeyDown={(event) => { if (['ArrowUp', 'PageUp', 'Home'].includes(event.key)) readingUp.current = true }}
        onPointerDown={(event) => { if (event.target === event.currentTarget) readingUp.current = true }}
      >
        {/* 아직 말이 없는 채팅방(홈): 한가운데에 몸소 CI 와 한 줄(2026-11-16 그의 말, Codex 앱의 빈 화면처럼). 첫 말을 보내면 사라진다.
            CI 는 docs/brand 의 메인 로고 agar yoga(인앱 로고) 심볼만, Codex 앱처럼 무채색으로(2026-11-16 그의 말 "심볼 넣을 떄 무채색으로").
            public/brand/agar-yoga-mono-mask.png 는 원본(agar-yoga.png = agar-yoga-mark-transparent.png)에서 만든 한 색 가림판 사본이다. */}
        {messages.length === 0 && !isGenerating && pendingApprovals.length === 0 && pendingSecrets.length === 0 && pendingChoices.length === 0 && notices.length === 0 && (
          <div className="chat-empty-hero">
            <span className="chat-empty-mark" aria-hidden="true" />
            <p className="chat-empty-title">{W.assistantPanel.emptyTitle}</p>
          </div>
        )}
        {messages.slice(shownFrom).map((message, offset) => {
          const index = shownFrom + offset
          return (
          <Fragment key={message.id}>
            {answeredAt(index).map(renderApproval)}
            {secretsDoneAt(index).map(renderSecret)}
            {choicesDoneAt(index).map(renderChoice)}
            {noticesAt(index).map(renderNotice)}
            <MessageItem
              message={message}
              always={message.role === 'assistant' && index === lastAssistantIndex && !isGenerating}
              hideEnd={Boolean(failure && !isGenerating && index === lastAssistantIndex && message.endReason === 'error')}
              onOpenDocument={openDocument}
              onOpenAddress={openAddress}
            />
          </Fragment>
          )
        })}
        {answeredAt(messages.length).map(renderApproval)}
        {secretsDoneAt(messages.length).map(renderSecret)}
        {choicesDoneAt(messages.length).map(renderChoice)}
        {noticesAt(messages.length).map(renderNotice)}
        {isGenerating && (
          <div className="message assistant generating">
            {streamText ? (
              // 흘러 들어오는 중에도 같은 도구로 그린다. 닫히지 않은 굵은 글씨·코드 조각은 닫힐 때까지 글자 그대로 둔다.
              <ZetMarkdown text={streamText} onOpenZet={onOpenDocument} onOpenAddress={openAddress} streaming />
            ) : (
              // 두 앱처럼 흐린 글 한 줄(도는 그림 없음).
              <span className="thinking">{W.assistantPanel.text3}</span>
            )}
          </div>
        )}
        {retrying && isGenerating && <div className="message-notice message-retrying" role="status">{retrying}</div>}
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
        {pendingChoices.map(renderChoice)}
      </div>

      {composer}
    </section>
  )
}

type MessageItemProps = {
  message: ChatViewMessage
  /** 마지막 답이면 복사 단추와 때를 늘 보인다. */
  always: boolean
  /** 답이 오지 않은 까닭 줄이 보이면 마지막 답의 "문제가 생겨 끊겼습니다"는 같은 말이라 두지 않는다. */
  hideEnd: boolean
  onOpenDocument: (id: string) => void
  onOpenAddress: (address: string) => void
}

/** 말 하나가 그리는 데 쓰는 값이 같으면 다시 그리지 않는다(App 이 그릴 때마다 말 목록을 새로 만들어도). */
function sameMessage(a: MessageItemProps, b: MessageItemProps) {
  const x = a.message
  const y = b.message
  return a.always === b.always && a.hideEnd === b.hideEnd && a.onOpenDocument === b.onOpenDocument && a.onOpenAddress === b.onOpenAddress
    && x.id === y.id && x.role === y.role && x.text === y.text && x.provider === y.provider && x.modelLabel === y.modelLabel
    && x.notHandedOver === y.notHandedOver && x.skill === y.skill && x.endReason === y.endReason && x.createdAt === y.createdAt
}

/** 대화의 말 하나: 넘기지 못한 말 알림, 내 말(말풍선) 또는 AI 의 답, 끊긴 답의 까닭 한 줄. */
const MessageItem = memo(function MessageItem({ message, always, hideEnd, onOpenDocument, onOpenAddress }: MessageItemProps) {
  return (
    <>
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
          {/* 붙여 보낸 첨부(2026-11-18 그의 말 "보낸 메시지에 같이 표시되어야"). 두 앱처럼 말풍선 위에 이름과 종류 그림. */}
          {message.attachments && message.attachments.length > 0 && (
            <ul className="message-attachments" aria-label={W.attach.sentLabel(message.attachments.length)}>
              {message.attachments.map((file) => {
                const Icon = file.mediaType?.startsWith('image/') ? ImageIcon : file.mediaType?.startsWith('audio/') ? AudioLines : file.mediaType?.startsWith('video/') ? Film : FileText
                return (
                  <li key={file.assetId} className={file.deleted ? 'message-attachment is-deleted' : 'message-attachment'} title={file.name}>
                    <Icon size={13} aria-hidden />
                    <span>{file.name}</span>
                  </li>
                )
              })}
            </ul>
          )}
          {message.text && <p>{message.text}</p>}
          {message.text && <MessageActions text={message.text} at={message.createdAt} side="user" />}
        </div>
      ) : (
        // AI 의 답: 말풍선 없이 바탕 위에 글. 답한 회사를 알면 머리에 그 회사 로고를 작게(올리면 모델 이름).
        <div className="message assistant" data-provider={message.provider ?? undefined}>
          {/* 가져온 대화의 답은 provider 가 세 회사가 아닌 값(예: 'chatgpt')으로 온다(2026-11-17 실제 앱). 모르는 값이면 로고를 그리지 않는다(화면 전체가 깨지지 않게). */}
          {message.provider && MARK[message.provider] && (
            <span className="message-by" title={answeredBy(message.provider, message.modelLabel)}>
              <ProviderMark provider={MARK[message.provider]} className="message-by-mark" />
              <span className="sr-only">{answeredBy(message.provider, message.modelLabel)}</span>
            </span>
          )}
          <ZetMarkdown text={message.text} onOpenZet={onOpenDocument} onOpenAddress={onOpenAddress} />
          {message.text && (
            <MessageActions text={message.text} at={message.createdAt} side="assistant" always={always} />
          )}
        </div>
      )}
      {/* 끊긴 답에만 그 아래 한 줄. 까닭은 답에 저장돼 앱을 다시 켜도 남는다. 모르는 값이면 적지 않는다. */}
      {/* 답이 오지 않은 까닭 줄(failure)이 보이면 마지막 답의 "문제가 생겨 끊겼습니다"는 같은 말이라 두지 않는다. */}
      {message.role === 'assistant' && message.endReason && W.assistantPanel.endReason[message.endReason]
        && !hideEnd && (
        <p className="message-notice message-end" role="note">{W.assistantPanel.endReason[message.endReason]}</p>
      )}
    </>
  )
}, sameMessage)

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
