import { W } from '../words'
import { useEffect, useRef, useState, type DragEvent } from 'react'
import { open as openNativeFileDialog } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  Bot,
  Check,
  FileAudio,
  FileText,
  FolderOpen,
  HardDrive,
  NotebookPen,
  Upload,
  X,
} from 'lucide-react'
import { getConversationKey, type AiProvider } from '../ai-conversation-import'
import { getRetentionLabel, type AppPreferences, type SourceRetention } from '../preferences'
import {
  applySourceAssetImport,
  isDesktopRuntime,
  planSourceAssetImport,
  readSourceTextPage,
  type SourceAssetImportPlan,
  type SourceAssetImportProposal,
} from '../runtime'
import {
  AUDIO_EXTENSIONS,
  SOURCE_IMPORT_ACTOR,
  TEXT_EXTENSIONS,
  assetKindForFileName,
  collectSourceText,
  describeImportPlan,
  fileNameFromPath,
  formatFileSize,
  isSupportedSourceName,
  isTextSourceName,
  mediaTypeForFileName,
  shortChecksum,
  splitParagraphs,
  stripExtension,
  summaryFromParagraphs,
  transcriptFromText,
} from '../source-import'
import type { SummarySection, TranscriptLine, WorkspaceRecord } from '../workbench-data'
import {
  AiConversationImporter,
  type AiConversationImporterHandle,
  type AiImportState,
} from './AiConversationImporter'

export type ImportMode = 'file' | 'ai' | 'note'

type ImportDialogProps = {
  open: boolean
  initialMode: ImportMode
  preferences: AppPreferences
  records: WorkspaceRecord[]
  onClose: () => void
  onCreateMany: (records: WorkspaceRecord[]) => Promise<WorkspaceRecord[]>
}

/** Windows 대화상자나 끌어 놓기로 받은 실제 경로와, Rust가 미리 계산한 보존 계획. */
type NativeSource = {
  path: string
  fileName: string
  plan: SourceAssetImportPlan
}

const ACCEPTED_FILES = [...AUDIO_EXTENSIONS, ...TEXT_EXTENSIONS].map((extension) => `.${extension}`).join(',')

function formatDuration(seconds: number) {
  const safeSeconds = Math.max(0, Math.round(seconds))
  const hours = Math.floor(safeSeconds / 3600)
  const minutes = Math.floor((safeSeconds % 3600) / 60)
  const remainingSeconds = safeSeconds % 60
  if (hours > 0) return W.importDialog.text1(hours, minutes, remainingSeconds)
  return W.importDialog.text2(minutes, remainingSeconds)
}

function isTextFile(file: File) {
  return file.type.startsWith('text/') || isTextSourceName(file.name)
}

async function getAudioDuration(file: File) {
  if (!file.type.startsWith('audio/') && !/\.(mp3|m4a|wav|aac|ogg|flac)$/i.test(file.name)) return null
  return new Promise<number | null>((resolve) => {
    const audio = document.createElement('audio')
    const objectUrl = URL.createObjectURL(file)
    const finish = (duration: number | null) => {
      URL.revokeObjectURL(objectUrl)
      resolve(duration)
    }
    audio.preload = 'metadata'
    audio.onloadedmetadata = () => finish(Number.isFinite(audio.duration) ? audio.duration : null)
    audio.onerror = () => finish(null)
    audio.src = objectUrl
  })
}

async function sha256(value: ArrayBuffer) {
  const digest = await crypto.subtle.digest('SHA-256', value)
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('')
}

function sourceProposal(source: NativeSource): SourceAssetImportProposal {
  return {
    sourcePath: source.path,
    assetKind: source.plan.assetKind,
    mediaType: mediaTypeForFileName(source.fileName),
    actor: SOURCE_IMPORT_ACTOR,
  }
}

const INITIAL_AI_IMPORT_STATE: AiImportState = {
  selectedCount: 0,
  canImport: false,
  isBusy: false,
}

export function ImportDialog({ open, initialMode, preferences, records, onClose, onCreateMany }: ImportDialogProps) {
  const [mode, setMode] = useState<ImportMode>('file')
  const [file, setFile] = useState<File | null>(null)
  const [nativeSource, setNativeSource] = useState<NativeSource | null>(null)
  const [isPlanning, setIsPlanning] = useState(false)
  const [title, setTitle] = useState('')
  const [participantLabel, setParticipantLabel] = useState(preferences.defaultParticipantLabel)
  const [note, setNote] = useState('')
  const [consentConfirmed, setConsentConfirmed] = useState(false)
  const [retention, setRetention] = useState<Exclude<SourceRetention, 'ask'>>('keep')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [error, setError] = useState('')
  const fileInputRef = useRef<HTMLInputElement>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const aiImporterRef = useRef<AiConversationImporterHandle>(null)
  const [aiImportState, setAiImportState] = useState<AiImportState>(INITIAL_AI_IMPORT_STATE)
  const desktopRuntime = isDesktopRuntime()

  useEffect(() => {
    if (!open) return
    const previousFocus = document.activeElement as HTMLElement | null
    setMode(initialMode)
    setFile(null)
    setNativeSource(null)
    setIsPlanning(false)
    setTitle('')
    setParticipantLabel(preferences.defaultParticipantLabel)
    setNote('')
    setConsentConfirmed(false)
    setRetention(preferences.sourceRetention === 'after-review' ? 'after-review' : 'keep')
    setAiImportState(INITIAL_AI_IMPORT_STATE)
    setError('')
    window.requestAnimationFrame(() => closeButtonRef.current?.focus())

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault()
        onClose()
        return
      }
      if (event.key !== 'Tab' || !dialogRef.current) return
      const focusable = dialogRef.current.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])',
      )
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => {
      document.removeEventListener('keydown', handleKeyDown)
      previousFocus?.focus()
    }
  }, [initialMode, open, onClose, preferences.defaultParticipantLabel, preferences.sourceRetention])

  // Tauri 창에서는 HTML drop 이벤트에 경로가 없으므로 웹뷰의 끌어 놓기 이벤트에서 경로를 받는다.
  useEffect(() => {
    if (!open || !desktopRuntime) return
    let disposed = false
    let unlisten: (() => void) | null = null
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== 'drop' || mode !== 'file') return
        const path = event.payload.paths[0]
        if (path) void planNativePath(path)
      })
      .then((stop) => {
        if (disposed) stop()
        else unlisten = stop
      })
      .catch(() => {
        // 끌어 놓기를 못 받아도 파일 선택 버튼으로 같은 경로를 고를 수 있다.
      })
    return () => {
      disposed = true
      unlisten?.()
    }
    // planNativePath는 상태 setter만 쓰므로 의존성에서 제외한다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, desktopRuntime, mode])

  if (!open) return null

  const selectedName = nativeSource?.fileName ?? file?.name ?? null
  const audioFile = selectedName ? !isTextSourceName(selectedName) && !(file && isTextFile(file)) : true
  const consentRequired = mode === 'file' && audioFile && preferences.confirmRecordingConsent
  const hasSource = Boolean(file || nativeSource)
  const canSubmit = mode === 'ai'
    ? aiImportState.canImport
    : mode === 'file'
      ? Boolean(hasSource && !isPlanning && title.trim() && participantLabel.trim() && (!consentRequired || consentConfirmed))
      : Boolean(title.trim() && participantLabel.trim() && note.trim())
  const existingImportKeys = records.flatMap((record) => (
    record.externalProvider && record.externalConversationId
      ? [getConversationKey({ provider: record.externalProvider as AiProvider, id: record.externalConversationId })]
      : []
  ))

  function chooseFile(nextFile: File | null) {
    if (!nextFile) return
    setFile(nextFile)
    setNativeSource(null)
    setTitle(stripExtension(nextFile.name))
    setError('')
  }

  async function planNativePath(path: string) {
    const fileName = fileNameFromPath(path)
    if (!isSupportedSourceName(fileName)) {
      setError(W.importDialog.text3(fileName, AUDIO_EXTENSIONS.join(', '), TEXT_EXTENSIONS.join(', ')))
      return
    }
    setIsPlanning(true)
    setError('')
    try {
      const plan = await planSourceAssetImport({
        sourcePath: path,
        assetKind: assetKindForFileName(fileName),
        mediaType: mediaTypeForFileName(fileName),
        actor: SOURCE_IMPORT_ACTOR,
      })
      setNativeSource({ path, fileName, plan })
      setFile(null)
      setTitle(stripExtension(fileName))
    } catch (planError) {
      setError(planError instanceof Error ? planError.message : String(planError))
    } finally {
      setIsPlanning(false)
    }
  }

  async function chooseNativeFile() {
    if (!desktopRuntime) {
      fileInputRef.current?.click()
      return
    }
    const selected = await openNativeFileDialog({
      title: W.importDialog.text4,
      multiple: false,
      directory: false,
      filters: [
        { name: W.importDialog.text5, extensions: [...AUDIO_EXTENSIONS, ...TEXT_EXTENSIONS] },
        { name: W.importDialog.text6, extensions: [...AUDIO_EXTENSIONS] },
        { name: W.importDialog.text7, extensions: [...TEXT_EXTENSIONS] },
      ],
    })
    if (typeof selected === 'string') await planNativePath(selected)
  }

  function handleDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault()
    chooseFile(event.dataTransfer.files[0] ?? null)
  }

  async function submit() {
    if (!canSubmit || isSubmitting) return
    if (mode === 'ai') {
      await aiImporterRef.current?.importSelected()
      return
    }
    setIsSubmitting(true)
    setError('')

    try {
      const now = new Date()
      const timestamp = now.toISOString().replace(/[-:TZ.]/g, '').slice(0, 14)
      const id = `${mode}-${timestamp}-${Math.random().toString(36).slice(2, 7)}`
      const createdAt = new Intl.DateTimeFormat('ko-KR', {
        dateStyle: 'long',
        timeStyle: 'short',
      }).format(now)
      const time = new Intl.DateTimeFormat('ko-KR', {
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      }).format(now)

      let sourceType: WorkspaceRecord['sourceType'] = 'note'
      let sourceLabel = W.importDialog.text8
      let duration = W.importDialog.text9
      let preview = note.trim().slice(0, 90)
      let hash = await sha256(new TextEncoder().encode(note.trim()).buffer)
      let transcript: TranscriptLine[] = []
      let summarySections: SummarySection[] = []
      let originalFileName: string | undefined
      let originalFileSize: string | undefined
      let archiveId = `momso-local / inbox-${timestamp}`
      let aiNote = W.importDialog.text10
      let linkSource: NativeSource | null = null

      if (mode === 'note') {
        const paragraphs = splitParagraphs(note)
        duration = W.importDialog.text11(paragraphs.length)
        transcript = paragraphs.map((text, index) => ({
          time: W.importDialog.text12(index + 1),
          speaker: W.importDialog.text13,
          text,
        }))
        summarySections = [{
          title: W.importDialog.text14,
          body: note.trim(),
          source: paragraphs.length > 1 ? W.importDialog.text15(paragraphs.length) : W.importDialog.text16,
        }]
      } else if (nativeSource) {
        // 1) 사람이 확인한 계획 그대로 원본을 해시 이름으로 보존한다.
        const asset = await applySourceAssetImport(sourceProposal(nativeSource), nativeSource.plan.planDigest, true)
        const { fileName, plan } = nativeSource
        originalFileName = fileName
        originalFileSize = formatFileSize(plan.byteLength)
        hash = plan.contentSha256
        archiveId = `momso-local / objects/${plan.contentSha256.slice(0, 2)}/${plan.contentSha256.slice(2, 10)}…`
        linkSource = nativeSource
        const fallbackBody = `${fileName} · ${formatFileSize(plan.byteLength)}`

        if (isTextSourceName(fileName)) {
          sourceType = 'note'
          sourceLabel = W.importDialog.text17
          try {
            // 2) 보존된 바이트를 해시로 묶어 끝까지 읽고, 그 문단을 그대로 기록 본문으로 옮긴다.
            const { text } = await collectSourceText(
              (cursor, expected) => readSourceTextPage(asset.id, cursor, expected),
            )
            const parsed = transcriptFromText(text)
            transcript = parsed.transcript
            duration = W.importDialog.text18(parsed.paragraphs.length)
            preview = (note.trim() || parsed.paragraphs[0] || fileName).slice(0, 90)
            summarySections = summaryFromParagraphs(parsed.paragraphs, fileName, note)
          } catch (readError) {
            const reason = readError instanceof Error ? readError.message : String(readError)
            duration = formatFileSize(plan.byteLength)
            preview = (note.trim() || fallbackBody).slice(0, 90)
            summarySections = [{ title: W.importDialog.text14, body: note.trim() || fallbackBody, source: W.importDialog.text19 }]
            aiNote = W.importDialog.text20(reason)
          }
        } else {
          sourceType = 'audio'
          sourceLabel = W.importDialog.text21
          duration = formatFileSize(plan.byteLength)
          preview = (note.trim() || fallbackBody).slice(0, 90)
          summarySections = [{ title: W.importDialog.text14, body: note.trim() || fallbackBody, source: W.importDialog.text19 }]
          aiNote = W.importDialog.text22
        }
      } else if (file) {
        // 설치된 앱이 아닌 브라우저 미리보기에서는 경로가 없어 내용 해시만 남긴다.
        originalFileName = file.name
        originalFileSize = formatFileSize(file.size)
        const bytes = await file.arrayBuffer()
        hash = await sha256(bytes)
        if (isTextFile(file)) {
          const text = new TextDecoder().decode(bytes)
          const parsed = transcriptFromText(text)
          sourceType = 'note'
          sourceLabel = W.importDialog.text17
          duration = W.importDialog.text18(parsed.paragraphs.length)
          preview = (note.trim() || parsed.paragraphs[0] || file.name).slice(0, 90)
          transcript = parsed.transcript
          summarySections = summaryFromParagraphs(parsed.paragraphs, file.name, note)
        } else {
          const audioDuration = await getAudioDuration(file)
          sourceType = 'audio'
          sourceLabel = W.importDialog.text21
          duration = audioDuration ? formatDuration(audioDuration) : formatFileSize(file.size)
          preview = (note.trim() || `${file.name} · ${formatFileSize(file.size)}`).slice(0, 90)
          summarySections = [{
            title: W.importDialog.text14,
            body: note.trim() || `${file.name} · ${formatFileSize(file.size)}`,
            source: W.importDialog.text19,
          }]
          aiNote = W.importDialog.text22
        }
      }

      const record: WorkspaceRecord = {
        id,
        title: title.trim(),
        preview,
        dateLabel: W.importDialog.text23,
        time,
        duration,
        sourceType,
        sourceLabel,
        participantLabel: participantLabel.trim(),
        status: 'needs-review',
        summarySections,
        transcript,
        createdAt,
        archiveId,
        checksum: shortChecksum(hash),
        aiNote,
        originalFileName,
        originalFileSize,
        retentionLabel: mode === 'file' ? getRetentionLabel(retention) : W.importDialog.text24,
      }
      const [stored] = await onCreateMany([record])

      // 3) 기록이 SQLite에 생긴 뒤에야 버전이 있으므로, 그 버전에 원본을 연결한다.
      if (linkSource && stored?.runtimeVersion) {
        const linkedProposal: SourceAssetImportProposal = {
          ...sourceProposal(linkSource),
          recordLink: { recordId: stored.id, recordVersion: stored.runtimeVersion, relationRole: 'source' },
        }
        try {
          const linkPlan = await planSourceAssetImport(linkedProposal)
          await applySourceAssetImport(linkedProposal, linkPlan.planDigest, true)
        } catch (linkError) {
          const reason = linkError instanceof Error ? linkError.message : String(linkError)
          setError(W.importDialog.text25(reason))
          return
        }
      }
      onClose()
    } catch (submitError) {
      setError(submitError instanceof Error ? submitError.message : String(submitError))
    } finally {
      setIsSubmitting(false)
    }
  }

  const selectedIsText = nativeSource ? isTextSourceName(nativeSource.fileName) : file ? isTextFile(file) : false

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div
        ref={dialogRef}
        className={mode === 'ai' ? 'import-dialog ai-import-dialog' : 'import-dialog'}
        role="dialog"
        aria-modal="true"
        aria-labelledby="import-dialog-title"
      >
        <header className="dialog-header">
          <div>
            <h2 id="import-dialog-title">{W.importDialog.text26}</h2>
          </div>
          <button ref={closeButtonRef} className="icon-button" type="button" onClick={onClose} aria-label={W.importDialog.aria1} title={W.importDialog.tip1}>
            <X size={18} aria-hidden />
          </button>
        </header>

        <div className="dialog-content">
          <div className="import-mode-control" role="tablist" aria-label={W.importDialog.aria2}>
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'file'}
              className={mode === 'file' ? 'active' : ''}
              onClick={() => { setMode('file'); setError('') }}
            >
              <Upload size={15} aria-hidden />{W.importDialog.text27}</button>
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'ai'}
              className={mode === 'ai' ? 'active' : ''}
              onClick={() => { setMode('ai'); setError('') }}
            >
              <Bot size={15} aria-hidden />{W.importDialog.text28}</button>
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'note'}
              className={mode === 'note' ? 'active' : ''}
              onClick={() => { setMode('note'); setError('') }}
            >
              <NotebookPen size={15} aria-hidden />{W.importDialog.text8}</button>
          </div>

          {mode === 'file' && (
            <div className="import-panel" role="tabpanel">
              <div
                className={hasSource ? 'file-dropzone has-file' : 'file-dropzone'}
                onDragOver={(event) => event.preventDefault()}
                onDrop={handleDrop}
              >
                <input
                  ref={fileInputRef}
                  className="sr-only"
                  type="file"
                  accept={ACCEPTED_FILES}
                  onChange={(event) => chooseFile(event.target.files?.[0] ?? null)}
                />
                {hasSource ? (
                  <>
                    <span className="selected-file-icon" aria-hidden>
                      {selectedIsText ? <FileText size={22} /> : <FileAudio size={22} />}
                    </span>
                    <div>
                      <strong>{selectedName}</strong>
                      <span>
                        {nativeSource
                          ? describeImportPlan(nativeSource.plan)
                          : file
                            ? formatFileSize(file.size)
                            : ''}
                      </span>
                    </div>
                    <button className="secondary-button" type="button" disabled={isPlanning} onClick={() => void chooseNativeFile()}>
                      <FolderOpen size={14} aria-hidden />{W.importDialog.text29}</button>
                  </>
                ) : (
                  <>
                    <span className="selected-file-icon" aria-hidden><HardDrive size={22} /></span>
                    <div>
                      <strong>{isPlanning ? W.importDialog.text30 : W.importDialog.text4}</strong>
                      <span>
                        {isPlanning
                          ? W.importDialog.text31
                          : W.importDialog.text32}
                      </span>
                    </div>
                    <button className="secondary-button" type="button" disabled={isPlanning} onClick={() => void chooseNativeFile()}>
                      <FolderOpen size={14} aria-hidden />{W.importDialog.text33}</button>
                  </>
                )}
              </div>

              <div className="import-form-grid">
                <label className="dialog-field wide">
                  <span>{W.importDialog.text34}</span>
                  <input value={title} maxLength={80} onChange={(event) => setTitle(event.target.value)} />
                </label>
                <label className="dialog-field">
                  <span>{W.importDialog.text35}</span>
                  <input value={participantLabel} maxLength={40} onChange={(event) => setParticipantLabel(event.target.value)} />
                </label>
                <label className="dialog-field">
                  <span>{W.importDialog.text36}</span>
                  <select value={retention} onChange={(event) => setRetention(event.target.value as Exclude<SourceRetention, 'ask'>)}>
                    <option value="keep">{getRetentionLabel('keep')}</option>
                    <option value="after-review">{getRetentionLabel('after-review')}</option>
                  </select>
                </label>
                <label className="dialog-field wide">
                  <span>{W.importDialog.text14}</span>
                  <textarea
                    value={note}
                    rows={3}
                    placeholder={W.importDialog.hint1}
                    onChange={(event) => setNote(event.target.value)}
                  />
                </label>
              </div>

              {consentRequired && (
                <label className={consentConfirmed ? 'consent-check checked' : 'consent-check'}>
                  <input
                    type="checkbox"
                    checked={consentConfirmed}
                    onChange={(event) => setConsentConfirmed(event.target.checked)}
                  />
                  <span className="consent-box" aria-hidden>{consentConfirmed && <Check size={13} />}</span>
                  <span>{W.importDialog.text37}</span>
                </label>
              )}
            </div>
          )}

          {mode === 'note' && (
            <div className="import-panel" role="tabpanel">
              <div className="import-form-grid note-form">
                <label className="dialog-field wide">
                  <span>{W.importDialog.text34}</span>
                  <input
                    value={title}
                    maxLength={80}
                    placeholder={W.importDialog.hint2}
                    onChange={(event) => setTitle(event.target.value)}
                  />
                </label>
                <label className="dialog-field wide">
                  <span>{W.importDialog.text35}</span>
                  <input value={participantLabel} maxLength={40} onChange={(event) => setParticipantLabel(event.target.value)} />
                </label>
                <label className="dialog-field wide">
                  <span>{W.importDialog.text38}</span>
                  <textarea
                    className="note-editor"
                    value={note}
                    rows={9}
                    placeholder={W.importDialog.hint3}
                    onChange={(event) => setNote(event.target.value)}
                  />
                </label>
              </div>
            </div>
          )}

          {mode === 'ai' && (
            <AiConversationImporter
              ref={aiImporterRef}
              existingImportKeys={existingImportKeys}
              onStateChange={setAiImportState}
              onImported={async (importedRecords) => {
                await onCreateMany(importedRecords)
                onClose()
              }}
            />
          )}

          {error && <div className="dialog-error" role="alert">{error}</div>}
        </div>

        <footer className="dialog-footer">
          <span>
            {mode === 'ai'
              ? aiImportState.selectedCount > 0
                ? W.importDialog.text39(aiImportState.selectedCount)
                : W.importDialog.text40
              : nativeSource
                ? W.importDialog.text41
                : W.importDialog.text42}
          </span>
          <div>
            <button className="secondary-button" type="button" onClick={onClose}>{W.importDialog.text43}</button>
            <button className="confirm-button" type="button" disabled={!canSubmit || isSubmitting} onClick={() => void submit()}>
              <Check size={15} aria-hidden />
              {mode === 'ai'
                ? aiImportState.isBusy
                  ? W.importDialog.text44
                  : aiImportState.selectedCount > 0
                    ? W.importDialog.text45(aiImportState.selectedCount)
                    : W.importDialog.text46
                : isSubmitting
                  ? nativeSource ? W.importDialog.text47 : W.importDialog.text48
                  : nativeSource ? W.importDialog.text49 : W.importDialog.text50}
            </button>
          </div>
        </footer>
      </div>
    </div>
  )
}
