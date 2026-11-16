import { Bot, Clock, FileAudio, FileText, Plus, Search } from 'lucide-react'
import {
  VIEW_LABELS,
  getStatusLabel,
  type QueueFilter,
  type WorkspaceRecord,
  type WorkspaceView,
} from '../workbench-data'

type TaskSidebarProps = {
  activeView: WorkspaceView
  workspaceName: string
  filter: QueueFilter
  query: string
  records: WorkspaceRecord[]
  selectedRecordId: string
  onFilterChange: (filter: QueueFilter) => void
  onQueryChange: (query: string) => void
  onRecordSelect: (recordId: string) => void
  onImport: () => void
}

const FILTERS: Array<{ id: QueueFilter; label: string }> = [
  { id: 'all', label: '전체' },
  { id: 'review', label: '검수' },
  { id: 'done', label: '완료' },
]

export function TaskSidebar({
  activeView,
  workspaceName,
  filter,
  query,
  records,
  selectedRecordId,
  onFilterChange,
  onQueryChange,
  onRecordSelect,
  onImport,
}: TaskSidebarProps) {
  const heading = VIEW_LABELS[activeView]

  return (
    <aside className="task-sidebar" aria-label={heading.title}>
      <header className="task-sidebar-header">
        <div className="product-lockup">
          <strong>momso</strong>
          <span>{workspaceName}</span>
        </div>
        <button className="icon-button" type="button" onClick={onImport} aria-label="기록 가져오기" title="기록 가져오기">
          <Plus size={18} aria-hidden />
        </button>
      </header>

      <div className="queue-heading">
        <h1>{heading.title}</h1>
      </div>

      <label className="queue-search">
        <Search size={15} aria-hidden />
        <span className="sr-only">기록 검색</span>
        <input
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="기록 검색"
          type="search"
        />
      </label>

      <div className="filter-control" role="group" aria-label="기록 상태 필터">
        {FILTERS.map((item) => (
          <button
            key={item.id}
            type="button"
            className={filter === item.id ? 'active' : ''}
            aria-pressed={filter === item.id}
            onClick={() => onFilterChange(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div className="record-list" aria-live="polite">
        {records.length === 0 ? (
          <div className="empty-queue">
            <span>표시할 기록이 없습니다</span>
          </div>
        ) : (
          records.map((record) => {
            const SourceIcon = record.sourceType === 'audio'
              ? FileAudio
              : record.sourceType === 'ai-conversation'
                ? Bot
                : FileText
            const isSelected = selectedRecordId === record.id
            return (
              <button
                key={record.id}
                className={isSelected ? 'record-row selected' : 'record-row'}
                type="button"
                aria-pressed={isSelected}
                onClick={() => onRecordSelect(record.id)}
              >
                <span className={`record-source-icon ${record.sourceType}`} aria-hidden>
                  <SourceIcon size={16} strokeWidth={1.75} />
                </span>
                <span className="record-row-body">
                  <span className="record-row-topline">
                    <strong>{record.title}</strong>
                    <time>{record.time}</time>
                  </span>
                  <span className="record-preview">{record.preview}</span>
                  <span className="record-row-meta">
                    <span className={`record-status ${record.status}`}>{getStatusLabel(record.status)}</span>
                    <span><Clock size={11} aria-hidden />{record.duration}</span>
                  </span>
                </span>
              </button>
            )
          })
        )}
      </div>

    </aside>
  )
}
