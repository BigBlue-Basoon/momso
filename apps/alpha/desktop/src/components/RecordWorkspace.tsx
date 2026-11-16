import { type KeyboardEvent, useEffect, useState } from 'react'
import {
  Calendar,
  Check,
  Clock,
  Bot,
  FileAudio,
  FileText,
  Pencil,
  Save,
  ShieldCheck,
  Users,
  X,
} from 'lucide-react'
import {
  getStatusLabel,
  type RecordTab,
  type SummarySection,
  type WorkspaceRecord,
} from '../workbench-data'

type RecordWorkspaceProps = {
  record: WorkspaceRecord
  activeTab: RecordTab
  onTabChange: (tab: RecordTab) => void
  onMarkReviewed: (recordId: string) => Promise<boolean>
  onUpdateSummary: (recordId: string, sections: SummarySection[]) => Promise<boolean>
}

const TABS: Array<{ id: RecordTab; label: string }> = [
  { id: 'summary', label: '정리본' },
  { id: 'transcript', label: '원문' },
  { id: 'details', label: '기록 정보' },
]

export function RecordWorkspace({
  record,
  activeTab,
  onTabChange,
  onMarkReviewed,
  onUpdateSummary,
}: RecordWorkspaceProps) {
  const [isEditing, setIsEditing] = useState(false)
  const [isSaving, setIsSaving] = useState(false)
  const [draftSections, setDraftSections] = useState(record.summarySections)
  const displayedSections = isEditing ? draftSections : record.summarySections

  useEffect(() => {
    setIsEditing(false)
    setDraftSections(record.summarySections)
  }, [record])

  async function saveDraft() {
    setIsSaving(true)
    try {
      if (await onUpdateSummary(record.id, draftSections)) setIsEditing(false)
    } finally {
      setIsSaving(false)
    }
  }

  async function markReviewed() {
    setIsSaving(true)
    try {
      await onMarkReviewed(record.id)
    } finally {
      setIsSaving(false)
    }
  }

  function handleTabKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    let nextIndex: number | null = null
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % TABS.length
    if (event.key === 'ArrowLeft') nextIndex = (index - 1 + TABS.length) % TABS.length
    if (event.key === 'Home') nextIndex = 0
    if (event.key === 'End') nextIndex = TABS.length - 1
    if (nextIndex === null) return

    event.preventDefault()
    onTabChange(TABS[nextIndex].id)
    const buttons = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('button')
    window.requestAnimationFrame(() => buttons?.[nextIndex]?.focus())
  }

  return (
    <main className="record-workspace">
      <header className="record-header">
        <div className="record-title-block">
          <h2>{record.title}</h2>
          <div className="record-header-meta">
            <span>
              {record.sourceType === 'ai-conversation'
                ? <Bot size={13} aria-hidden />
                : record.sourceType === 'audio'
                  ? <FileAudio size={13} aria-hidden />
                  : <FileText size={13} aria-hidden />}
              {record.sourceLabel}
            </span>
            <span><Calendar size={13} aria-hidden />{record.createdAt}</span>
            <span><Clock size={13} aria-hidden />{record.duration}</span>
            <span><Users size={13} aria-hidden />{record.participantLabel}</span>
          </div>
        </div>

        <div className="record-actions">
          {isEditing ? (
            <>
              <button
                className="secondary-button"
                type="button"
                disabled={isSaving}
                onClick={() => {
                  setDraftSections(record.summarySections)
                  setIsEditing(false)
                }}
              >
                <X size={15} aria-hidden />
                취소
              </button>
              <button className="secondary-button" type="button" disabled={isSaving} onClick={() => void saveDraft()}>
                <Save size={15} aria-hidden />
                {isSaving ? '저장 중' : '저장'}
              </button>
            </>
          ) : (
            <button className="secondary-button" type="button" onClick={() => setIsEditing(true)}>
              <Pencil size={15} aria-hidden />
              수정
            </button>
          )}
          <button
            className="confirm-button"
            type="button"
            disabled={record.status === 'reviewed' || isSaving}
            onClick={() => void markReviewed()}
          >
            <Check size={16} aria-hidden />
            {record.status === 'reviewed' ? '검수 완료' : '확정'}
          </button>
        </div>
      </header>

      <div className="record-tabbar">
        <div
          className="glass-segmented record-tabs"
          data-active={activeTab}
          role="tablist"
          aria-label="기록 보기"
        >
          <span className="glass-slider" aria-hidden />
          {TABS.map((tab, index) => (
            <button
              key={tab.id}
              id={`record-tab-${tab.id}`}
              type="button"
              role="tab"
              aria-selected={activeTab === tab.id}
              aria-controls={`record-panel-${tab.id}`}
              tabIndex={activeTab === tab.id ? 0 : -1}
              className={activeTab === tab.id ? 'active' : ''}
              onClick={() => onTabChange(tab.id)}
              onKeyDown={(event) => handleTabKeyDown(event, index)}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </div>

      <div className="record-canvas">
        {activeTab === 'summary' && (
          <article
            id="record-panel-summary"
            className="summary-document"
            role="tabpanel"
            aria-labelledby="record-tab-summary"
          >
            <header className="document-header">
              <div>
                <h3>수업 기록 정리</h3>
              </div>
              <span className={`document-state ${record.status}`}>
                {record.status === 'reviewed' ? <ShieldCheck size={14} aria-hidden /> : <FileText size={14} aria-hidden />}
                {getStatusLabel(record.status)}
              </span>
            </header>

            <div className="summary-sections">
              {displayedSections.map((section, index) => (
                <section key={section.title} className="summary-section">
                  <div className="section-content">
                    <h4>{section.title}</h4>
                    {isEditing ? (
                      <textarea
                        value={section.body}
                        aria-label={`${section.title} 수정`}
                        onChange={(event) => {
                          const nextSections = [...draftSections]
                          nextSections[index] = { ...section, body: event.target.value }
                          setDraftSections(nextSections)
                        }}
                      />
                    ) : (
                      <p>{section.body}</p>
                    )}
                    <button className="source-link" type="button" onClick={() => onTabChange('transcript')}>
                      <FileText size={13} aria-hidden />
                      원문 {section.source}
                    </button>
                  </div>
                </section>
              ))}
            </div>

            <footer className="document-footer">
              <ShieldCheck size={15} aria-hidden />
              <span>원문 표현과 검수 주체를 보존한 로컬 초안</span>
            </footer>
          </article>
        )}

        {activeTab === 'transcript' && (
          <section
            id="record-panel-transcript"
            className="transcript-view"
            role="tabpanel"
            aria-labelledby="record-tab-transcript"
          >
            <header className="view-heading">
              <div>
                <h3>{record.sourceLabel}</h3>
              </div>
              <span className="source-duration">
                {record.sourceType === 'ai-conversation' ? <Bot size={15} aria-hidden /> : <FileAudio size={15} aria-hidden />}
                {record.duration}
              </span>
            </header>
            <div className="transcript-lines">
              {record.transcript.length > 0 ? (
                record.transcript.map((line, index) => (
                  <div key={`${record.id}-${line.time}-${index}`} className={line.important ? 'transcript-line important' : 'transcript-line'}>
                    <time>{line.time}</time>
                    <strong>{line.speaker}</strong>
                    <p>{line.text}</p>
                  </div>
                ))
              ) : (
                <div className="empty-source">
                  <FileAudio size={22} aria-hidden />
                  <strong>텍스트 원문 없음</strong>
                  <span>이 기록은 오디오 원본과 기록 메모로 구성되어 있습니다.</span>
                </div>
              )}
            </div>
          </section>
        )}

        {activeTab === 'details' && (
          <section
            id="record-panel-details"
            className="details-view"
            role="tabpanel"
            aria-labelledby="record-tab-details"
          >
            <header className="view-heading">
              <div>
                <h3>기록 정보</h3>
              </div>
            </header>
            <dl className="details-list">
              <div><dt>원본 유형</dt><dd>{record.sourceLabel}</dd></div>
              <div><dt>생성 시각</dt><dd>{record.createdAt}</dd></div>
              <div><dt>로컬 식별자</dt><dd>{record.archiveId}</dd></div>
              <div><dt>원본 체크섬</dt><dd>{record.checksum}</dd></div>
              {record.originalFileName && <div><dt>원본 파일</dt><dd>{record.originalFileName}</dd></div>}
              {record.originalFileSize && <div><dt>파일 크기</dt><dd>{record.originalFileSize}</dd></div>}
              {record.retentionLabel && <div><dt>원본 보관</dt><dd>{record.retentionLabel}</dd></div>}
              {record.externalProvider && <div><dt>가져온 서비스</dt><dd>{record.sourceLabel}</dd></div>}
              {record.projectLabel && <div><dt>원본 프로젝트</dt><dd>{record.projectLabel}</dd></div>}
              {record.externalConversationId && <div><dt>원본 대화 ID</dt><dd>{record.externalConversationId}</dd></div>}
              <div><dt>검수 상태</dt><dd>{getStatusLabel(record.status)}</dd></div>
              <div><dt>저장 위치</dt><dd>이 기기의 momso 작업공간</dd></div>
            </dl>
          </section>
        )}
      </div>
    </main>
  )
}
