import { W } from '../words'
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from 'react'
import {
  Bot,
  Check,
  FileArchive,
  FileJson,
  FolderSearch,
  LoaderCircle,
  MessageSquareText,
  Search,
} from 'lucide-react'
import {
  AI_PROVIDER_INFO,
  conversationToWorkspaceRecord,
  getConversationKey,
  parseAiConversationFiles,
  previewConversation,
  type AiConversation,
  type AiConversationPreview,
  type AiProvider,
} from '../ai-conversation-import'
import {
  discoverLocalAiConversations,
  isDesktopRuntime,
  loadLocalAiConversations,
} from '../runtime'
import type { WorkspaceRecord } from '../workbench-data'

type Candidate = {
  preview: AiConversationPreview
  conversation?: AiConversation
}

export type AiImportState = {
  selectedCount: number
  canImport: boolean
  isBusy: boolean
}

export type AiConversationImporterHandle = {
  importSelected: () => Promise<void>
}

type AiConversationImporterProps = {
  existingImportKeys: string[]
  onImported: (records: WorkspaceRecord[]) => Promise<void>
  onStateChange: (state: AiImportState) => void
}

const PROVIDERS: Array<{ id: AiProvider; monogram: string }> = [
  { id: 'codex', monogram: 'CX' },
  { id: 'claude-code', monogram: 'CC' },
  { id: 'chatgpt', monogram: 'GP' },
  { id: 'claude', monogram: 'CL' },
]

const AI_IMPORT_FILES = '.zip,.json,.jsonl,.md,.txt'

function formatConversationDate(value: string | undefined) {
  if (!value) return W.aiConversationImporter.text1
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat('ko-KR', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function mergeCandidates(current: Candidate[], incoming: Candidate[]) {
  const merged = new Map(current.map((candidate) => [getConversationKey(candidate.preview), candidate]))
  for (const candidate of incoming) merged.set(getConversationKey(candidate.preview), candidate)
  return [...merged.values()].sort((left, right) => {
    const leftTime = new Date(left.preview.updatedAt ?? left.preview.createdAt ?? 0).getTime()
    const rightTime = new Date(right.preview.updatedAt ?? right.preview.createdAt ?? 0).getTime()
    return rightTime - leftTime
  })
}

export const AiConversationImporter = forwardRef<AiConversationImporterHandle, AiConversationImporterProps>(
  function AiConversationImporter({ existingImportKeys, onImported, onStateChange }, ref) {
    const [provider, setProvider] = useState<AiProvider>('codex')
    const [candidates, setCandidates] = useState<Candidate[]>([])
    const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set())
    const [focusedKey, setFocusedKey] = useState('')
    const [query, setQuery] = useState('')
    const [isBusy, setIsBusy] = useState(false)
    const [error, setError] = useState('')
    const fileInputRef = useRef<HTMLInputElement>(null)
    const existingSet = useMemo(() => new Set(existingImportKeys), [existingImportKeys])

    const filteredCandidates = useMemo(() => {
      const normalized = query.trim().toLocaleLowerCase('ko-KR')
      if (!normalized) return candidates
      return candidates.filter(({ preview }) => (
        `${preview.title} ${preview.projectLabel ?? ''} ${AI_PROVIDER_INFO[preview.provider].label}`
          .toLocaleLowerCase('ko-KR')
          .includes(normalized)
      ))
    }, [candidates, query])

    const selectableFilteredKeys = filteredCandidates
      .map(({ preview }) => getConversationKey(preview))
      .filter((key) => !existingSet.has(key))
    const selectedCount = [...selectedKeys].filter((key) => !existingSet.has(key)).length
    const focusedCandidate = candidates.find(({ preview }) => getConversationKey(preview) === focusedKey)
      ?? filteredCandidates[0]
      ?? candidates[0]

    useEffect(() => {
      onStateChange({
        selectedCount,
        canImport: selectedCount > 0 && !isBusy,
        isBusy,
      })
    }, [isBusy, onStateChange, selectedCount])

    function resetForProvider(nextProvider: AiProvider) {
      setProvider(nextProvider)
      setCandidates([])
      setSelectedKeys(new Set())
      setFocusedKey('')
      setQuery('')
      setError('')
      if (fileInputRef.current) fileInputRef.current.value = ''
    }

    function acceptCandidates(incoming: Candidate[]) {
      const next = mergeCandidates(candidates, incoming)
      setCandidates(next)
      const incomingKeys = incoming
        .map(({ preview }) => getConversationKey(preview))
        .filter((key) => !existingSet.has(key))
      setSelectedKeys((current) => new Set([...current, ...incomingKeys]))
      if (!focusedKey && incoming[0]) setFocusedKey(getConversationKey(incoming[0].preview))
    }

    async function discoverOnComputer() {
      setIsBusy(true)
      setError('')
      try {
        const previews = await discoverLocalAiConversations(provider)
        if (previews.length === 0) {
          setError(W.aiConversationImporter.text2(AI_PROVIDER_INFO[provider].label))
          return
        }
        acceptCandidates(previews.map((preview) => ({ preview })))
      } catch (discoveryError) {
        setError(discoveryError instanceof Error ? discoveryError.message : String(discoveryError))
      } finally {
        setIsBusy(false)
      }
    }

    async function readFiles(files: File[]) {
      if (files.length === 0) return
      setIsBusy(true)
      setError('')
      try {
        const conversations = await parseAiConversationFiles(files, provider)
        acceptCandidates(conversations.map((conversation) => ({
          preview: previewConversation(conversation),
          conversation,
        })))
      } catch (fileError) {
        setError(fileError instanceof Error ? fileError.message : String(fileError))
      } finally {
        setIsBusy(false)
        if (fileInputRef.current) fileInputRef.current.value = ''
      }
    }

    function toggleCandidate(key: string) {
      if (existingSet.has(key)) return
      setSelectedKeys((current) => {
        const next = new Set(current)
        if (next.has(key)) next.delete(key)
        else next.add(key)
        return next
      })
    }

    function toggleAllVisible() {
      const allSelected = selectableFilteredKeys.length > 0
        && selectableFilteredKeys.every((key) => selectedKeys.has(key))
      setSelectedKeys((current) => {
        const next = new Set(current)
        for (const key of selectableFilteredKeys) {
          if (allSelected) next.delete(key)
          else next.add(key)
        }
        return next
      })
    }

    useImperativeHandle(ref, () => ({
      async importSelected() {
        const selectedCandidates = candidates.filter(({ preview }) => {
          const key = getConversationKey(preview)
          return selectedKeys.has(key) && !existingSet.has(key)
        })
        if (selectedCandidates.length === 0 || isBusy) return

        setIsBusy(true)
        setError('')
        try {
          const conversations = selectedCandidates.flatMap((candidate) => (
            candidate.conversation ? [candidate.conversation] : []
          ))
          const localCandidates = selectedCandidates.filter((candidate) => !candidate.conversation)
          const providers = [...new Set(localCandidates.map((candidate) => candidate.preview.provider))]
          for (const localProvider of providers) {
            const locators = localCandidates
              .filter((candidate) => candidate.preview.provider === localProvider)
              .map((candidate) => candidate.preview.sourceLocator)
              .filter((locator): locator is string => Boolean(locator))
            conversations.push(...await loadLocalAiConversations(localProvider, locators))
          }
          const records = await Promise.all(conversations.map(conversationToWorkspaceRecord))
          if (records.length === 0) throw new Error(W.aiConversationImporter.text3)
          await onImported(records)
        } catch (importError) {
          setError(importError instanceof Error ? importError.message : String(importError))
        } finally {
          setIsBusy(false)
        }
      },
    }), [candidates, existingSet, isBusy, onImported, selectedKeys])

    const providerInfo = AI_PROVIDER_INFO[provider]
    const allVisibleSelected = selectableFilteredKeys.length > 0
      && selectableFilteredKeys.every((key) => selectedKeys.has(key))

    return (
      <div className="ai-import-panel" role="tabpanel">
        <div className="ai-provider-strip" role="list" aria-label={W.aiConversationImporter.aria1}>
          {PROVIDERS.map((item) => (
            <button
              key={item.id}
              type="button"
              className={provider === item.id ? 'ai-provider-button active' : 'ai-provider-button'}
              aria-pressed={provider === item.id}
              onClick={() => resetForProvider(item.id)}
            >
              <span aria-hidden>{item.monogram}</span>
              {AI_PROVIDER_INFO[item.id].label}
            </button>
          ))}
        </div>

        <div className="ai-source-toolbar">
          <div>
            <strong>{providerInfo.label}</strong>
            <span>{providerInfo.fileHint}</span>
          </div>
          <div>
            {providerInfo.localDiscovery && isDesktopRuntime() && (
              <button className="secondary-button" type="button" disabled={isBusy} onClick={() => void discoverOnComputer()}>
                <FolderSearch size={15} aria-hidden />{W.aiConversationImporter.text4}</button>
            )}
            <input
              ref={fileInputRef}
              className="sr-only"
              type="file"
              accept={AI_IMPORT_FILES}
              multiple
              onChange={(event) => void readFiles([...event.target.files ?? []])}
            />
            <button className="secondary-button" type="button" disabled={isBusy} onClick={() => fileInputRef.current?.click()}>
              {provider === 'chatgpt' ? <FileArchive size={15} aria-hidden /> : <FileJson size={15} aria-hidden />}{W.aiConversationImporter.text5}</button>
          </div>
        </div>

        {isBusy && candidates.length === 0 ? (
          <div className="ai-import-empty" aria-live="polite">
            <LoaderCircle className="spin" size={24} aria-hidden />
            <strong>{W.aiConversationImporter.text6}</strong>
          </div>
        ) : candidates.length === 0 ? (
          <div className="ai-import-empty">
            <Bot size={25} aria-hidden />
            <strong>{W.aiConversationImporter.text7(providerInfo.label)}</strong>
            <span>{W.aiConversationImporter.text8}</span>
          </div>
        ) : (
          <div className="ai-conversation-browser">
            <section className="ai-conversation-list" aria-label={W.aiConversationImporter.aria2}>
              <header>
                <label className="ai-select-all">
                  <input
                    type="checkbox"
                    checked={allVisibleSelected}
                    onChange={toggleAllVisible}
                  />
                  <span>{W.aiConversationImporter.text9}</span>
                </label>
                <span>{W.aiConversationImporter.text10(filteredCandidates.length)}</span>
              </header>
              <label className="ai-conversation-search">
                <Search size={14} aria-hidden />
                <span className="sr-only">{W.aiConversationImporter.text11}</span>
                <input
                  type="search"
                  value={query}
                  placeholder={W.aiConversationImporter.hint1}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </label>
              <div className="ai-conversation-scroll">
                {filteredCandidates.map(({ preview }) => {
                  const key = getConversationKey(preview)
                  const isDuplicate = existingSet.has(key)
                  const isFocused = focusedCandidate && getConversationKey(focusedCandidate.preview) === key
                  return (
                    <div key={key} className={isFocused ? 'ai-conversation-row focused' : 'ai-conversation-row'}>
                      <input
                        type="checkbox"
                        aria-label={W.aiConversationImporter.aria3(preview.title)}
                        checked={selectedKeys.has(key) && !isDuplicate}
                        disabled={isDuplicate}
                        onChange={() => toggleCandidate(key)}
                      />
                      <button type="button" onClick={() => setFocusedKey(key)}>
                        <strong>{preview.title}</strong>
                        <span>{W.aiConversationImporter.text12(preview.projectLabel ?? AI_PROVIDER_INFO[preview.provider].label, preview.messageCount)}</span>
                      </button>
                      {isDuplicate && <span className="duplicate-label"><Check size={11} aria-hidden />{W.aiConversationImporter.text13}</span>}
                    </div>
                  )
                })}
                {filteredCandidates.length === 0 && <div className="ai-list-empty">{W.aiConversationImporter.text14}</div>}
              </div>
            </section>

            <section className="ai-conversation-preview" aria-label={W.aiConversationImporter.aria4}>
              {focusedCandidate ? (
                <>
                  <header>
                    <h3>{focusedCandidate.preview.title}</h3>
                    <p>
                      {AI_PROVIDER_INFO[focusedCandidate.preview.provider].label}
                      {' · '}
                      {formatConversationDate(focusedCandidate.preview.updatedAt ?? focusedCandidate.preview.createdAt)}
                    </p>
                  </header>
                  <div className="ai-preview-messages">
                    {focusedCandidate.preview.sampleMessages.map((message, index) => (
                      <div key={`${focusedCandidate.preview.id}-${index}`} className={`ai-preview-message ${message.role}`}>
                        <span>{message.role === 'user' ? W.aiConversationImporter.text15 : AI_PROVIDER_INFO[focusedCandidate.preview.provider].shortLabel}</span>
                        <p>{message.text}</p>
                      </div>
                    ))}
                  </div>
                  <footer>
                    <MessageSquareText size={14} aria-hidden />{W.aiConversationImporter.text16}{focusedCandidate.preview.messageCount}{W.aiConversationImporter.text17}</footer>
                </>
              ) : (
                <div className="ai-import-empty"><MessageSquareText size={24} aria-hidden /></div>
              )}
            </section>
          </div>
        )}

        {error && <div className="dialog-error" role="alert">{error}</div>}
      </div>
    )
  },
)
