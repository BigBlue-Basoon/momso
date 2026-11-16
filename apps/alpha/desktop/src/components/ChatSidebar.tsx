import { type DragEvent, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Archive, ChevronDown, ChevronRight, FolderPlus, MoreHorizontal, Pin, PinOff, Search, SquarePen } from 'lucide-react'
import type { ChatEntry, ChatGroup } from '../chats'
import { GROUP_WORD, W } from '../words'
import { ShortcutTip } from './ShortcutTip'
import { QuestionDialog } from './ZetPanel'
import './ChatSidebar.css'

const FOLDED_KEY = 'momso.desktop.group-folded.v1'
const DRAG_TYPE = 'text/momso-chat'

type ChatSidebarProps = {
  chats: ChatEntry[]
  /** 허락을 구하는 물음에 답을 기다리는 채팅방. 줄 끝에 작은 점이 붙는다. */
  waitingChatIds?: string[]
  /** 고른 채팅방. 새 채팅방을 쓰는 중(아직 저장 전)이면 null. */
  selectedChatId: string | null
  /** 목록을 읽지 못했으면 그 까닭. */
  error: string | null
  onNewChat: () => void
  onSelectChat: (id: string) => void
  /** 채팅방 묶음. null 이면 묶음 백엔드가 없다(묶음 만들기가 눌리지 않는다, 글 없음). */
  groups: ChatGroup[] | null
  onCreateGroup: () => Promise<string | null>
  onRenameGroup: (id: string, name: string) => void
  onMoveGroup: (id: string, step: -1 | 1) => void
  onRemoveGroup: (id: string) => void
  onSetGroup: (chatId: string, groupId: string | null) => void
  /** "검색" 단추: 검색 창을 연다(Ctrl+K). */
  onOpenSearch: () => void
  /**
   * 채팅방 고정·보관. 없으면(명령이 없음) 단추를 그리지 않는다. Codex 앱처럼 줄에 올리면 줄 끝에 고정·보관 단추가 보이고,
   * 고정한 채팅방은 목록 맨 위 "고정된 채팅방"에 모인다(프로젝트 안이든 밖이든 그 자리에는 다시 그리지 않는다).
   */
  onPinChat?: (chatId: string, pinned: boolean) => void
  onArchiveChat?: (chatId: string) => void
  /** 답을 쓰는 중인 채팅방(보관 단추를 그리지 않는다). */
  busyChatId?: string | null
}

function loadFolded(): Record<string, boolean> {
  try { return JSON.parse(window.localStorage.getItem(FOLDED_KEY) ?? '{}') as Record<string, boolean> } catch { return {} }
}

/**
 * 왼쪽 채팅방 목록. 맨 위에 "새 채팅방"과 묶음 만들기, 찾기. 그 아래 묶음마다 머리(접고 펴기)와 그 안의 채팅방,
 * 그 아래 묶이지 않은 채팅방(얇은 선으로 가름). 채팅방은 끌어서 묶음 머리에 놓거나 줄의 메뉴로 넣고 뺀다.
 * 찾기는 묶음과 상관없이 전부에서 찾고, 결과에 어느 묶음인지 보인다. 지우기는 목록에 없고, 보관한 채팅방을 설정에서 영구 삭제한다.
 */
export function ChatSidebar({
  chats,
  waitingChatIds = [],
  selectedChatId,
  error,
  onNewChat,
  onSelectChat,
  groups,
  onCreateGroup,
  onRenameGroup,
  onMoveGroup,
  onRemoveGroup,
  onSetGroup,
  onOpenSearch,
  onPinChat,
  onArchiveChat,
  busyChatId = null,
}: ChatSidebarProps) {
  const [folded, setFolded] = useState(loadFolded)
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [removing, setRemoving] = useState<ChatGroup | null>(null)
  const [dropTarget, setDropTarget] = useState<string | null>(null)
  const toggleFold = (id: string) => {
    setFolded((current) => {
      const next = { ...current, [id]: !current[id] }
      try { window.localStorage.setItem(FOLDED_KEY, JSON.stringify(next)) } catch { /* 기억하지 못해도 지금은 접힌다 */ }
      return next
    })
  }

  const orderedGroups = [...(groups ?? [])].sort((a, b) => a.order - b.order)
  const groupIds = new Set(orderedGroups.map((group) => group.id))
  // 고정한 채팅방은 맨 위에만 그린다(같은 줄을 두 번 두지 않는다).
  const pinned = onPinChat ? chats.filter((chat) => chat.pinned) : []
  const listed = pinned.length > 0 ? chats.filter((chat) => !chat.pinned) : chats
  const ungrouped = listed.filter((chat) => !chat.groupId || !groupIds.has(chat.groupId))

  const renderRow = (chat: ChatEntry) => (
    <ChatRow
      key={chat.id}
      chat={chat}
      selected={chat.id === selectedChatId}
      waiting={waitingChatIds.includes(chat.id)}
      groups={groups ? orderedGroups : null}
      onSelect={() => onSelectChat(chat.id)}
      onSetGroup={(groupId) => onSetGroup(chat.id, groupId)}
      onPin={onPinChat ? () => onPinChat(chat.id, !chat.pinned) : undefined}
      onArchive={onArchiveChat && chat.id !== busyChatId ? () => onArchiveChat(chat.id) : undefined}
    />
  )

  const dropProps = (target: string) => ({
    onDragOver: (event: DragEvent) => {
      if (!groups || !event.dataTransfer.types.includes(DRAG_TYPE)) return
      event.preventDefault()
      setDropTarget(target)
    },
    onDragLeave: () => setDropTarget((current) => (current === target ? null : current)),
    onDrop: (event: DragEvent) => {
      const chatId = event.dataTransfer.getData(DRAG_TYPE)
      setDropTarget(null)
      if (chatId) onSetGroup(chatId, target === 'ungrouped' ? null : target)
    },
  })

  return (
    <aside className="task-sidebar chat-sidebar" aria-label={W.chatSidebar.aria1}>
      <div className="chat-sidebar-tools">
        <div className="chat-new-row">
          <button className="chat-new" type="button" onClick={onNewChat}>
            <SquarePen size={16} strokeWidth={1.75} aria-hidden />{W.chatSidebar.text1}</button>
          <ShortcutTip label={W.chatSidebar.label1(GROUP_WORD)}>
            <button
              className="chat-group-add"
              type="button"
              aria-label={W.chatSidebar.label1(GROUP_WORD)}
              aria-disabled={!groups || undefined}
              onClick={() => {
                if (!groups) return
                void onCreateGroup().then((id) => { if (id) setRenamingId(id) })
              }}
            >
              <FolderPlus size={16} strokeWidth={1.75} aria-hidden />
            </button>
          </ShortcutTip>
        </div>
        <ShortcutTip label={W.search} keys={['Ctrl', 'K']} side="right">
          <button className="chat-new chat-search-button" type="button" onClick={onOpenSearch}>
            <Search size={16} strokeWidth={1.75} aria-hidden />
            {W.search}
          </button>
        </ShortcutTip>
      </div>

      <div className="record-list chat-list" aria-live="polite">
        {error ? (
          <div className="empty-queue"><span>{error}</span></div>
        ) : (
          <>
            {pinned.length > 0 && (
              <section className="chat-pinned" aria-label={W.chatKeep.pinnedHeading}>
                <p className="chat-pinned-head">{W.chatKeep.pinnedHeading}</p>
                <ul className="chat-rows">{pinned.map(renderRow)}</ul>
              </section>
            )}
            {orderedGroups.map((group, index) => {
              const inside = listed.filter((chat) => chat.groupId === group.id)
              const isFolded = Boolean(folded[group.id])
              return (
                <section key={group.id} className="chat-group" aria-label={group.name}>
                  <GroupHeader
                    group={group}
                    count={inside.length}
                    folded={isFolded}
                    renaming={renamingId === group.id}
                    dropping={dropTarget === group.id}
                    first={index === 0}
                    last={index === orderedGroups.length - 1}
                    dropProps={dropProps(group.id)}
                    onToggle={() => toggleFold(group.id)}
                    onStartRename={() => setRenamingId(group.id)}
                    onRename={(name) => {
                      setRenamingId(null)
                      if (name && name !== group.name) onRenameGroup(group.id, name)
                    }}
                    onMove={(step) => onMoveGroup(group.id, step)}
                    onRemove={() => setRemoving(group)}
                  />
                  {!isFolded && inside.length > 0 && <ul className="chat-rows chat-group-rows">{inside.map(renderRow)}</ul>}
                </section>
              )
            })}
            {ungrouped.length > 0 && (
              <ul
                className={`chat-rows${orderedGroups.length > 0 || pinned.length > 0 ? ' chat-ungrouped' : ''}${dropTarget === 'ungrouped' ? ' is-drop' : ''}`}
                {...dropProps('ungrouped')}
              >
                {ungrouped.map(renderRow)}
              </ul>
            )}
          </>
        )}
      </div>

      {removing && (
        <QuestionDialog
          title={W.chatSidebar.title1(removing.name, GROUP_WORD)}
          text={W.chatSidebar.text2(GROUP_WORD)}
          actions={[
            { label: W.chatSidebar.text3, onSelect: () => setRemoving(null), autoFocus: true },
            { label: W.chatSidebar.text4(GROUP_WORD), primary: true, onSelect: () => { onRemoveGroup(removing.id); setRemoving(null) } },
          ]}
          onDismiss={() => setRemoving(null)}
        />
      )}
    </aside>
  )
}

/** 줄 끝 "…" 단추가 여는 작은 메뉴. 단추 아래 오른쪽 끝에 맞춰 띄운다. */
function RowMenu({ label, children }: { label: string; children: (close: () => void) => ReactNode }) {
  const [open, setOpen] = useState(false)
  const button = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const [place, setPlace] = useState({ top: 0, left: 0 })

  useLayoutEffect(() => {
    if (!open || !button.current) return
    const rect = button.current.getBoundingClientRect()
    const width = menu.current?.offsetWidth ?? 180
    const height = menu.current?.offsetHeight ?? 120
    const top = rect.bottom + 4 + height > window.innerHeight - 8 ? rect.top - height - 4 : rect.bottom + 4
    setPlace({ top, left: Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8)) })
  }, [open])

  useEffect(() => {
    if (!open) return
    const close = (event: MouseEvent) => {
      const target = event.target as Node
      if (menu.current?.contains(target) || button.current?.contains(target)) return
      setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setOpen(false); button.current?.focus() }
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  return (
    <>
      <button
        ref={button}
        className="chat-more"
        type="button"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(event) => { event.stopPropagation(); setOpen((current) => !current) }}
      >
        <MoreHorizontal size={15} aria-hidden />
      </button>
      {open && createPortal(
        <div ref={menu} className="chat-menu" role="menu" aria-label={label} style={{ top: place.top, left: place.left }}>
          {children(() => setOpen(false))}
        </div>,
        document.body,
      )}
    </>
  )
}

function MenuItem({ children, onSelect, close }: { children: ReactNode; onSelect: () => void; close: () => void }) {
  return (
    <button className="chat-menu-item" type="button" role="menuitem" onClick={() => { close(); onSelect() }}>
      {children}
    </button>
  )
}

function GroupHeader({
  group,
  count,
  folded,
  renaming,
  dropping,
  first,
  last,
  dropProps,
  onToggle,
  onStartRename,
  onRename,
  onMove,
  onRemove,
}: {
  group: ChatGroup
  count: number
  folded: boolean
  renaming: boolean
  dropping: boolean
  first: boolean
  last: boolean
  dropProps: Record<string, unknown>
  onToggle: () => void
  onStartRename: () => void
  onRename: (name: string) => void
  onMove: (step: -1 | 1) => void
  onRemove: () => void
}) {
  const [draft, setDraft] = useState(group.name)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (renaming) {
      setDraft(group.name)
      window.requestAnimationFrame(() => input.current?.select())
    }
  }, [renaming, group.name])

  return (
    <div className={`chat-group-head${dropping ? ' is-drop' : ''}`} {...dropProps}>
      {renaming ? (
        <input
          ref={input}
          className="chat-group-input"
          value={draft}
          maxLength={40}
          aria-label={W.chatSidebar.aria2(GROUP_WORD)}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={() => onRename(draft.trim())}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.nativeEvent.isComposing) onRename(draft.trim())
            if (event.key === 'Escape') onRename(group.name)
          }}
        />
      ) : (
        <button className="chat-group-toggle" type="button" aria-expanded={!folded} onClick={onToggle} onDoubleClick={onStartRename}>
          {folded ? <ChevronRight size={14} aria-hidden /> : <ChevronDown size={14} aria-hidden />}
          <span className="chat-group-name">{group.name}</span>
          <span className="chat-group-count">{count}</span>
        </button>
      )}
      {!renaming && (
        <RowMenu label={W.chatSidebar.label2(group.name, GROUP_WORD)}>
          {(close) => (
            <>
              <MenuItem close={close} onSelect={onStartRename}>{W.chatSidebar.text5}</MenuItem>
              {!first && <MenuItem close={close} onSelect={() => onMove(-1)}>{W.chatSidebar.text6}</MenuItem>}
              {!last && <MenuItem close={close} onSelect={() => onMove(1)}>{W.chatSidebar.text7}</MenuItem>}
              <MenuItem close={close} onSelect={onRemove}>{W.chatSidebar.text4(GROUP_WORD)}</MenuItem>
            </>
          )}
        </RowMenu>
      )}
    </div>
  )
}

function ChatRow({
  chat,
  selected,
  waiting,
  groups,
  onSelect,
  onSetGroup,
  onPin,
  onArchive,
}: {
  chat: ChatEntry
  selected: boolean
  waiting: boolean
  groups: ChatGroup[] | null
  onSelect: () => void
  onSetGroup: (groupId: string | null) => void
  onPin?: () => void
  onArchive?: () => void
}) {
  const title = chat.title || W.chatSidebar.text1
  const pinLabel = chat.pinned ? W.chatKeep.unpin : W.chatKeep.pin
  return (
    <li className={`chat-row-item${onPin || onArchive ? ' has-keep' : ''}${groups && groups.length > 0 ? ' has-more' : ''}`}>
      <button
        className={`chat-row${selected ? ' selected' : ''}${waiting ? ' is-waiting' : ''}`}
        type="button"
        draggable={Boolean(groups && groups.length > 0)}
        onDragStart={(event) => {
          event.dataTransfer.setData(DRAG_TYPE, chat.id)
          event.dataTransfer.effectAllowed = 'move'
        }}
        aria-current={selected ? 'true' : undefined}
        aria-label={waiting ? W.chatSidebar.aria3(title) : undefined}
        title={title}
        onClick={onSelect}
      >
        <span className="chat-row-title">{title}</span>
        {waiting && <span className="chat-row-waiting" aria-hidden="true" />}
      </button>
      {(onPin || onArchive) && (
        <span className="chat-keep">
          {onPin && (
            <ShortcutTip label={pinLabel}>
              <button className={`chat-keep-button${chat.pinned ? ' is-pinned' : ''}`} type="button" aria-label={pinLabel} onClick={(event) => { event.stopPropagation(); onPin() }}>
                {chat.pinned ? <PinOff size={14} aria-hidden /> : <Pin size={14} aria-hidden />}
              </button>
            </ShortcutTip>
          )}
          {onArchive && (
            <ShortcutTip label={W.chatKeep.archive}>
              <button className="chat-keep-button" type="button" aria-label={W.chatKeep.archive} onClick={(event) => { event.stopPropagation(); onArchive() }}>
                <Archive size={14} aria-hidden />
              </button>
            </ShortcutTip>
          )}
        </span>
      )}
      {groups && groups.length > 0 && (
        <RowMenu label={W.chatSidebar.label3(title)}>
          {(close) => (
            <>
              <p className="chat-menu-label">{W.chatSidebar.text8(GROUP_WORD)}</p>
              {groups.filter((group) => group.id !== chat.groupId).map((group) => (
                <MenuItem key={group.id} close={close} onSelect={() => onSetGroup(group.id)}>{group.name}</MenuItem>
              ))}
              {chat.groupId && <MenuItem close={close} onSelect={() => onSetGroup(null)}>{W.chatSidebar.text9(GROUP_WORD)}</MenuItem>}
            </>
          )}
        </RowMenu>
      )}
    </li>
  )
}
