import { W } from './words'
// AI 가 허락을 구할 때의 물음(백엔드 approval.rs). 모양은 archive-backend-port-notes.md 의 "승인 응답 공통 처리"와 "T31 승인 카드 계약"을 따른다.
//   이벤트 agent-approval-request: { requestId, chatId, provider, kind, description, change, targets, reason, options }
//     change: create / edit / overwrite / delete / null. 대상마다 종류가 다르면 null 이고 targets 의 각 change 를 읽는다.
//     코덱스는 값이 없으면 change null, targets []. null 이면 종류를 짐작해 적지 않는다.
//   이벤트 agent-approval-closed:  { requestId, chatId }
//   명령 agent_approval_respond:   { requestId, choice } → null
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AiProvider } from './ai-models'
import { isDesktopRuntime } from './runtime'

export type ApprovalChoice = 'allow_once' | 'allow_session' | 'deny' | 'cancel'
export type ApprovalKind = 'command' | 'file_change' | 'other'
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

export function orderedChoices(options: ApprovalChoice[]) {
  return CHOICE_ORDER.filter((choice) => options.includes(choice))
}

export async function respondApproval(requestId: string, choice: ApprovalChoice) {
  if (!isDesktopRuntime()) throw new Error('agent_approval_respond needs the desktop runtime')
  await invoke<null>('agent_approval_respond', { requestId, choice })
}

export async function listenApprovals(handlers: {
  onRequest: (request: ApprovalRequest) => void
  onClosed: (requestId: string) => void
}) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<ApprovalRequest>('agent-approval-request', (event) => handlers.onRequest(event.payload)),
    listen<{ requestId: string }>('agent-approval-closed', (event) => handlers.onClosed(event.payload.requestId)),
  ])
  return () => stops.forEach((stop) => stop())
}
