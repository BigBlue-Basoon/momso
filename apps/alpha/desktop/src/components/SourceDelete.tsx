import { useEffect, useRef, useState } from 'react'
import { readZet } from '../archive'
import { listChats } from '../chats'
import { previewSourceDelete, purgeSource, restoreTrashedSource, sourceErrorCode, trashSource, type SourceDeletePreview } from '../source-assets'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

export type NoticeAction = { label: string; run: () => void }
/** 알림 줄. 단추는 하나 또는 둘(2026-11-18: 다운로드 알림의 [채팅방에 첨부] [폴더에서 보기]). */
export type SourceNotice = (text: string, action?: NoticeAction | NoticeAction[]) => void

type Uses = { documents: string[]; chats: string[]; receipts: number; records: number }

/**
 * 잘못 첨부한 원자료를 지운다(2026-11-17 본부 지시). 먼저 뒤쪽이 그 원자료를 쓰는 곳(문서·작업 내역·채팅방)을 알려 주면 그것을 보이고 묻는다.
 * "삭제"는 되살릴 수 있게 지우고(30일 넘게 보관) 알림 줄에서 바로 되돌릴 수 있다. "완전히 삭제"는 한 번 더 묻고,
 * 뒤쪽이 남을 수 있다고 돌려준 사본(연결한 저장소·다른 곳)은 그대로 알린다.
 */
export function SourceDeleteDialog({ assetId, name, onClose, onNotice, onChanged }: {
  assetId: string
  name: string
  onClose: () => void
  onNotice: SourceNotice
  onChanged: () => void
}) {
  const [step, setStep] = useState<'checking' | 'ask' | 'purge'>('checking')
  const [working, setWorking] = useState(false)
  const [preview, setPreview] = useState<SourceDeletePreview | null>(null)
  const [uses, setUses] = useState<Uses | null>(null)
  const [changed, setChanged] = useState(false)
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])

  async function check(again: boolean) {
    setStep('checking')
    try {
      const value = await previewSourceDelete(assetId)
      // 쓰는 곳의 이름: 문서는 제목, 채팅방은 이름(읽지 못한 것은 빼고 수만 센다).
      const [titles, chats] = await Promise.all([
        Promise.all(value.documents.map((id) => readZet(id).then((doc) => doc.head.title).catch(() => null))),
        value.chats.length > 0 ? listChats().catch(() => []) : Promise.resolve([]),
      ])
      if (!alive.current) return
      const chatTitle = new Map(chats.map((chat) => [chat.id, chat.title]))
      setPreview(value)
      setUses({
        documents: titles.map((title) => title ?? W.sourceDelete.unnamedDocument),
        chats: value.chats.map((id) => chatTitle.get(id) || W.chatRoom),
        receipts: value.receipts.length,
        records: value.recordVersions.length,
      })
      setChanged(again)
      setStep('ask')
    } catch {
      if (!alive.current) return
      onNotice(W.sourceDelete.checkFailed)
      onClose()
    }
  }
  useEffect(() => { void check(false) }, [assetId]) // eslint-disable-line react-hooks/exhaustive-deps

  async function run(kind: 'trash' | 'purge') {
    if (!preview) return
    setWorking(true)
    try {
      if (kind === 'trash') {
        await trashSource(preview.assetId, preview.planDigest)
        const id = preview.assetId
        onChanged()
        onNotice(W.sourceDelete.trashed, {
          label: W.sourceDelete.undo,
          run: () => { restoreTrashedSource(id).then(() => { onChanged(); onNotice(W.sourceDelete.restored) }).catch(() => onNotice(W.sourceDelete.restoreFailed)) },
        })
      } else {
        const result = await purgeSource(preview.assetId, preview.planDigest)
        onChanged()
        const left = result.state === 'partial' || result.remoteRemaining.length > 0 || result.localRemaining.length > 0
        onNotice(left ? W.sourceDelete.purgedLeft : W.sourceDelete.purged)
      }
      onClose()
    } catch (error) {
      const code = sourceErrorCode(error)
      setWorking(false)
      if (code === 'references_changed') { void check(true); return }
      onNotice(code === 'chat_active' ? W.sourceDelete.busy : W.sourceDelete.failed)
      onClose()
    }
  }

  if (step === 'checking' || !uses) {
    return <QuestionDialog key="checking" title={W.sourceDelete.title(name)} text={W.sourceDelete.checking} actions={[{ label: W.signIn.cancel, onSelect: onClose, autoFocus: true }]} onDismiss={onClose} />
  }
  if (step === 'purge') {
    return (
      <QuestionDialog
        key="purge"
        title={W.sourceDelete.purgeTitle(name)}
        text={W.sourceDelete.purgeText}
        working={working}
        actions={[
          { label: W.signIn.cancel, onSelect: onClose, autoFocus: true },
          { label: W.sourceDelete.purge, primary: true, onSelect: () => void run('purge') },
        ]}
        onDismiss={onClose}
      />
    )
  }
  const parts = [
    uses.documents.length > 0 && W.sourceDelete.documents(uses.documents.length),
    uses.receipts > 0 && W.sourceDelete.receipts(uses.receipts),
    uses.chats.length > 0 && W.sourceDelete.chats(uses.chats.length),
    uses.records > 0 && W.sourceDelete.records(uses.records),
  ].filter(Boolean) as string[]
  const names = [...uses.documents, ...uses.chats]
  return (
    // 단계마다 key 를 달리 해 창을 새로 그린다(같은 창을 다시 쓰면 "그만두기"에 초점이 가지 않는다).
    <QuestionDialog
      key={changed ? 'ask-again' : 'ask'}
      title={W.sourceDelete.title(name)}
      text={[
        changed ? W.sourceDelete.changed : null,
        parts.length > 0 ? W.sourceDelete.usedBy(parts.join(', ')) : W.sourceDelete.unused,
        W.sourceDelete.trashText,
      ].filter(Boolean).join(' ')}
      detail={names.length > 0 ? (
        <ul className="source-delete-uses">
          {names.slice(0, 6).map((title, index) => <li key={`${index}-${title}`}>{title}</li>)}
          {names.length > 6 && <li>{W.sourceDelete.more(names.length - 6)}</li>}
        </ul>
      ) : undefined}
      working={working}
      actions={[
        { label: W.signIn.cancel, onSelect: onClose, autoFocus: true },
        { label: W.sourceDelete.purge, onSelect: () => setStep('purge') },
        { label: W.sourceDelete.trash, primary: true, onSelect: () => void run('trash') },
      ]}
      onDismiss={onClose}
    />
  )
}
