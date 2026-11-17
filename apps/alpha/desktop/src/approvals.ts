import { W } from './words'
// AI 가 허락을 구할 때의 물음(백엔드 approval.rs). 모양은 archive-backend-port-notes.md 의 "승인 응답 공통 처리"와 "T31 승인 카드 계약"을 따른다.
//   이벤트 agent-approval-request: { requestId, chatId, provider, kind, description, change, targets, reason, options }
//     change: create / edit / overwrite / delete / null. 대상마다 종류가 다르면 null 이고 targets 의 각 change 를 읽는다.
//     코덱스는 값이 없으면 change null, targets []. null 이면 종류를 짐작해 적지 않는다.
//   이벤트 agent-approval-closed:  { requestId, chatId, reason }
//     reason(2026-11-17): responded(답함) / timeout(5분 동안 답 없음) / provider_cancelled(AI 쪽이 거둠·대화 멈춤) / auto_declined(몸소가 받치지 않는 코덱스 요청을 스스로 거절). 옛 값은 없음.
//   명령 agent_approval_respond:   { requestId, choice } → null
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AiProvider } from './ai-models'
import { isDesktopRuntime } from './runtime'

export type ApprovalChoice = 'allow_once' | 'allow_session' | 'deny' | 'cancel'
export type ApprovalKind = 'command' | 'file_change' | 'setup_change' | 'browser_action_scope' | 'browser_action' | 'other'

/** 몸소 브라우저의 물음(browser.ts 의 계약). 누르기·입력 한 번마다 묻는 일이라 "이번만 허락"과 "거절"만 둔다. */
export function isBrowserApproval(request?: ApprovalRequest) {
  return request?.kind === 'browser_action_scope' || request?.kind === 'browser_action'
}

/**
 * 스킬("GitHub 연결", "원자료 저장소 연결")로 AI 가 몸소의 도구를 불러 무언가를 만들고 연결하려 할 때의 물음.
 * 모양은 archive-backend-port-notes.md "2026-11-16 옮긴 것"의 C9 설정 승인 계약(화면 폴더에 옮겨짐, cef940fa):
 *   kind 'setup_change', setup { action:'create_and_connect', service:'github'|'cloudflare'|'gcp'|'aws', account, region, resourceName,
 *   visibility:'private', cost:'possible' }, targets 에 만들 이름과 change 'create', options allow_once·deny·cancel,
 *   답은 지금과 같은 agent_approval_respond({ requestId, choice }).
 * 화면은 "이번만 허락"과 "거절"만 보인다(2026-11-16 본부): "이 채팅방에서 계속 허락"(allow_session)과 "그만두기"는 options 에 와도 두지 않는다.
 */
export type SetupAction = {
  action: 'create_and_connect' | string
  service: string
  account?: string | null
  region?: string | null
  resourceName?: string | null
  visibility?: 'private' | string | null
  cost?: 'possible' | string | null
}
export type ApprovalChange = 'create' | 'edit' | 'overwrite' | 'delete'
export type ApprovalTarget = { path: string; change: ApprovalChange | null }

export type ApprovalRequest = {
  requestId: string
  provider: AiProvider | string
  kind: ApprovalKind | string
  /** 명령 글자 또는 바뀌는 파일 경로, 그 밖에는 하려는 일. */
  description: string
  /** AI 가 밝힌 까닭(있을 때만). */
  reason: string | null
  /** 그 AI 가 받는 답만 온다. 온 것만 단추로 보인다. */
  options: ApprovalChoice[]
  /** 물음이 온 채팅방(없으면 답을 쓰는 중인 대화). */
  chatId?: string | null
  /** 파일을 어떻게 바꾸려는지. 모르거나 대상마다 다르면 null. */
  change?: ApprovalChange | null
  /** 바뀌는 파일들(모르면 빈 목록). */
  targets?: ApprovalTarget[] | null
  /** kind 가 'setup_change' 일 때 하려는 일(위 SetupAction). */
  setup?: SetupAction | null
}

/**
 * 물음 전체의 바꾸는 종류: 맨 위 change, 없으면 대상이 하나일 때 그 대상의 것. 모르면 null(짐작하지 않는다).
 */
export function approvalChange(request: ApprovalRequest): ApprovalChange | null {
  if (request.change) return request.change
  const targets = request.targets ?? []
  return targets.length === 1 ? targets[0].change ?? null : null
}

/** 단추 글. 차례도 이대로 보인다. */
export const CHOICE_LABEL: Record<ApprovalChoice, string> = {
  allow_once: W.approvals.text1,
  allow_session: W.approvals.text2,
  deny: W.approvals.text3,
  cancel: W.approvals.text4,
}

/** 답한 뒤 대화에 남는 한 줄. */
export const CHOICE_DONE: Record<ApprovalChoice, string> = {
  allow_once: W.approvals.text5,
  allow_session: W.approvals.text6,
  deny: W.approvals.text7,
  cancel: W.approvals.text8,
}

const CHOICE_ORDER: ApprovalChoice[] = ['allow_once', 'allow_session', 'deny', 'cancel']

export function orderedChoices(options: ApprovalChoice[], request?: ApprovalRequest) {
  // 설정 승인과 브라우저 승인은 한 번 한 번 묻는 일이다: "이 채팅방에서 계속 허락"(allow_session)과 "그만두기"는 와도 두지 않는다.
  const once = request?.kind === 'setup_change' || isBrowserApproval(request)
  return CHOICE_ORDER.filter((choice) => options.includes(choice) && !(once && (choice === 'allow_session' || choice === 'cancel')))
}

export async function respondApproval(requestId: string, choice: ApprovalChoice) {
  if (!isDesktopRuntime()) throw new Error('agent_approval_respond needs the desktop runtime')
  await invoke<null>('agent_approval_respond', { requestId, choice })
}

export async function listenApprovals(handlers: {
  onRequest: (request: ApprovalRequest) => void
  onClosed: (requestId: string, reason: string | null) => void
}) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<ApprovalRequest>('agent-approval-request', (event) => handlers.onRequest(event.payload)),
    listen<{ requestId: string; reason?: string | null }>('agent-approval-closed', (event) => handlers.onClosed(event.payload.requestId, typeof event.payload.reason === 'string' ? event.payload.reason : null)),
  ])
  return () => stops.forEach((stop) => stop())
}
