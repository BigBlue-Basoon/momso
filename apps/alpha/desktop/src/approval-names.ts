import { W } from './words'
import type { ApprovalRequest } from './approvals'

/** 원자료(objects/sha256/…) 파일인지. 해시 조각은 사람이 읽을 이름이 아니다(2026-11-19 G81 5). */
export function isSourcePath(path: string) {
  return /(^|[\\/])objects[\\/]sha256[\\/]/i.test(path)
}

export function fileName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

/**
 * 대상 하나의 보이는 이름. 원자료면 뒤쪽이 준 원래 파일 이름(originalFileName, G82), 없으면 "원자료 파일"(짐작하지 않는다).
 * 그 밖은 파일 이름. originalFileName 은 대상이 하나일 때만 오므로 여럿일 때는 넘기지 않는다.
 */
export function targetName(path: string, originalFileName?: string | null) {
  if (!isSourcePath(path)) return fileName(path)
  return originalFileName?.trim() || W.approvalCard.sourceFile
}

/**
 * 문서 만들기 물음의 폴더 이름(G82 folder). "" 는 맨 위라 문서 화면의 맨 위 이름(W.document)과 같이,
 * 그 밖은 문서 화면이 옮긴 곳을 알릴 때처럼 마지막 폴더 이름. null·값 없음이면 null(폴더를 지어내지 않는다).
 */
export function approvalFolderName(request: Pick<ApprovalRequest, 'folder'>) {
  const folder = request.folder
  if (typeof folder !== 'string') return null
  const parts = folder.split(/[\\/]/).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : W.document
}
