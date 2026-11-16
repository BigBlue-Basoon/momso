import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from 'react'
import { Plus, Search, X } from 'lucide-react'
import {
  deleteYogaPerson,
  deleteYogaTerm,
  dictionaryErrorCode,
  editYogaPerson,
  editYogaTerm,
  listenDictionaryUpdated,
  listYogaTerms,
  mySpellings,
  normalizeQuery,
  personSearchText,
  searchText,
  shownSpellings,
  type DictionaryList,
  type DictionaryPerson,
  type DictionaryTerm,
} from '../dictionary'
import { archiveErrorMessage } from '../archive'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

type Tab = 'terms' | 'people'

/** 누른 단추가 사라지거나 잠겨 포커스가 화면 밖(body)으로 빠졌을 때만 다음 자리로 옮긴다(키보드만으로 쓸 때). */
function focusIfLost(target: () => HTMLElement | null | undefined) {
  requestAnimationFrame(() => {
    const active = document.activeElement
    if (!active || active === document.body) target()?.focus()
  })
}

/**
 * 설정 › 용어 사전. 2026-11-16 그의 결정: 이름은 "용어 사전", 여러 표기를 다 적어 알아보는 데 쓴다. 모양은 그가 정한 적이 없어 가장 작게 얹었다(그가 보고 고친다).
 * 위에 "용어 / 사람 이름" 두 칸과 찾기 칸. 용어 줄을 누르면 그 아래에서 뜻 수정, 표기 더하기, 내가 더한 표기 삭제, 이 용어 쓰기(끄기)를 한다.
 * 고친 것은 뒤쪽이 내 파일(my-terms.json, people.json)에만 쓰고 기본 파일은 바뀌지 않는다. 용어를 지우는 명령은 없어서 끄기만 둔다.
 * 사람 이름은 더하기·수정·삭제와 "확인됨으로 바꾸기"(AI 가 더한 이름은 확인 안 됨으로 들어온다). 찾기는 화면에 보이지 않는 표기(searchOnly)까지 본다.
 * 출처 주소와 "사람 검수 전" 같은 사전의 상태는 보이지 않는다.
 */
export function TermDictionarySettings() {
  const [data, setData] = useState<DictionaryList | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [tab, setTab] = useState<Tab>('terms')
  const [openId, setOpenId] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [working, setWorking] = useState(false)
  const [message, setMessage] = useState<{ text: string; alert: boolean } | null>(null)
  const deferred = useDeferredValue(query)
  const words = W.termDictionary
  const addButton = useRef<HTMLButtonElement>(null)
  const focusAdd = () => focusIfLost(() => addButton.current)

  const reload = useCallback(async () => {
    try {
      const next = await listYogaTerms()
      setData(next)
      setProblem(null)
      return next
    } catch (error) {
      setProblem(archiveErrorMessage(error, words.failed))
      return null
    }
  }, [words.failed])

  useEffect(() => {
    let alive = true
    let stop: (() => void) | undefined
    void reload()
    void listenDictionaryUpdated(() => { if (alive) void reload() }).then((off) => { if (alive) stop = off; else off() })
    return () => { alive = false; stop?.() }
  }, [reload])

  /** 쓰기 한 번: 끝나면 목록을 다시 읽어 새 revision 을 받는다. 그사이 바뀌었으면(dictionary_changed) 다시 읽고 알린다. */
  async function write(run: (current: DictionaryList) => Promise<unknown>) {
    if (!data || working) return false
    setWorking(true)
    setMessage(null)
    try {
      await run(data)
      await reload()
      setMessage({ text: words.saved, alert: false })
      return true
    } catch (error) {
      const code = dictionaryErrorCode(error)
      if (code === 'dictionary_changed') {
        await reload()
        setMessage({ text: words.changed, alert: true })
      } else {
        setMessage({ text: code === 'invalid_input' ? words.invalidInput : code === 'base_term' ? words.baseTerm : words.saveFailed, alert: true })
      }
      return false
    } finally {
      setWorking(false)
    }
  }

  const termIndex = useMemo(() => (data?.terms ?? []).map((term) => ({ term, text: searchText(term), spellings: shownSpellings(term) })), [data])
  const groups = useMemo(() => {
    const needle = normalizeQuery(deferred)
    const found = needle ? termIndex.filter((item) => item.text.includes(needle)) : termIndex
    const byCategory = new Map<string, typeof found>()
    for (const item of found) {
      const key = item.term.category?.trim() || words.otherCategory
      byCategory.set(key, [...(byCategory.get(key) ?? []), item])
    }
    return { count: found.length, list: [...byCategory.entries()] }
  }, [termIndex, deferred, words.otherCategory])
  // 새 용어의 갈래: 지금 화면이 묶어 보이는 갈래(뒤쪽 기본값 '사용자'는 뺀다).
  const categories = useMemo(() => [...new Set((data?.terms ?? []).map((term) => term.category?.trim() ?? '').filter((name) => name && name !== '사용자'))], [data])
  const people = useMemo(() => {
    const needle = normalizeQuery(deferred)
    const list = data?.people ?? []
    return needle ? list.filter((person) => personSearchText(person).includes(needle)) : list
  }, [data, deferred])

  function switchTab(next: Tab) {
    setTab(next)
    setQuery('')
    setAdding(false)
    setOpenId(null)
    setMessage(null)
  }

  const peopleBroken = data?.peopleStatus === 'invalid'

  return (
    <div className="settings-page term-dictionary">
      <header className="settings-page-heading skills-heading">
        <div>
          <h2>{words.title}</h2>
          <p>{words.intro}</p>
        </div>
        {data && !(tab === 'people' && peopleBroken) && (
          <button ref={addButton} className="secondary-button" type="button" onClick={() => { setAdding(true); setOpenId(null); setMessage(null) }}>
            <Plus size={14} aria-hidden />{tab === 'terms' ? words.addTerm : words.addPerson}
          </button>
        )}
      </header>
      <div className="term-tabs" role="tablist" aria-label={words.title}>
        <button type="button" role="tab" aria-selected={tab === 'terms'} className={tab === 'terms' ? 'is-active' : ''} onClick={() => switchTab('terms')}>{words.termsTab}</button>
        <button type="button" role="tab" aria-selected={tab === 'people'} className={tab === 'people' ? 'is-active' : ''} onClick={() => switchTab('people')}>{words.peopleTab}</button>
      </div>
      <label className="term-search">
        <Search size={15} aria-hidden />
        <input
          value={query}
          placeholder={tab === 'terms' ? words.searchHint : words.peopleSearchHint}
          aria-label={tab === 'terms' ? words.searchHint : words.peopleSearchHint}
          spellCheck={false}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      {message && <p className={message.alert ? 'term-note is-alert' : 'term-note'} role={message.alert ? 'alert' : 'status'}>{message.text}</p>}
      {problem ? (
        <p className="term-note" role="alert">{problem}</p>
      ) : data === null ? (
        <p className="term-note" role="status">{words.loading}</p>
      ) : tab === 'terms' ? (
        <>
          {adding && (
            <NewTermForm
              working={working}
              categories={categories}
              onCancel={() => { setAdding(false); focusAdd() }}
              onSave={async (spelling, meaning, category) => {
                const ok = await write((current) => editYogaTerm({ id: null, addVariants: [spelling], ...(meaning ? { meaning } : {}), ...(category ? { category } : {}), expectedRevision: current.userRevision }))
                if (ok) { setAdding(false); focusAdd() }
              }}
            />
          )}
          <p className="term-count" role="status">{query.trim() ? words.found(groups.count) : words.total(data.terms.length)}</p>
          {groups.count === 0 && <p className="term-note">{words.empty}</p>}
          {groups.list.map(([category, items]) => (
            <section key={category} className="term-group" aria-label={category}>
              <h3>{category}</h3>
              <ul className="term-rows">
                {items.map(({ term, spellings }) => (
                  <TermRow
                    key={term.id}
                    term={term}
                    spellings={spellings}
                    open={openId === term.id}
                    working={working}
                    onToggle={() => { setOpenId(openId === term.id ? null : term.id); setAdding(false); setMessage(null) }}
                    onEdit={(change) => write((current) => editYogaTerm({ id: term.id, ...change, expectedRevision: current.userRevision }))}
                    onDelete={async () => {
                      const ok = await write((current) => deleteYogaTerm(term.id, current.userRevision))
                      if (ok) { setOpenId(null); focusAdd() }
                    }}
                  />
                ))}
              </ul>
            </section>
          ))}
        </>
      ) : peopleBroken ? (
        <p className="term-note" role="alert">{words.peopleInvalid}</p>
      ) : (
        <PeopleList
          people={people}
          total={data.people.length}
          searching={Boolean(query.trim())}
          adding={adding}
          working={working}
          onStopAdding={() => { setAdding(false); focusAdd() }}
          onFocusAdd={focusAdd}
          onSave={(person, next) => write((current) => editYogaPerson({ id: person?.id ?? null, ...next, expectedRevision: current.peopleRevision }))}
          onDelete={(person) => write((current) => deleteYogaPerson(person.id ?? '', current.peopleRevision))}
        />
      )}
    </div>
  )
}

function NewTermForm({ working, categories, onCancel, onSave }: { working: boolean; categories: string[]; onCancel: () => void; onSave: (spelling: string, meaning: string, category: string) => void }) {
  const words = W.termDictionary
  const [spelling, setSpelling] = useState('')
  const [meaning, setMeaning] = useState('')
  const [category, setCategory] = useState('')
  return (
    <form className="term-form" onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel() } }} onSubmit={(event) => { event.preventDefault(); if (spelling.trim()) onSave(spelling.trim(), meaning.trim(), category) }}>
      <label className="setting-field">
        <span>{words.fieldSpelling}</span>
        <input value={spelling} onChange={(event) => setSpelling(event.target.value)} maxLength={100} autoFocus />
      </label>
      <label className="setting-field">
        <span>{words.fieldMeaning}</span>
        <input value={meaning} onChange={(event) => setMeaning(event.target.value)} maxLength={500} />
      </label>
      <label className="setting-field">
        <span>{words.fieldCategory}</span>
        <select value={category} onChange={(event) => setCategory(event.target.value)}>
          <option value="">{words.categoryNone}</option>
          {categories.map((name) => <option key={name} value={name}>{name}</option>)}
        </select>
      </label>
      <div className="term-form-actions">
        <button className="secondary-button" type="button" onClick={onCancel}>{words.cancel}</button>
        <button className="confirm-button" type="submit" disabled={working || !spelling.trim()}>{words.save}</button>
      </div>
    </form>
  )
}

type TermChange = { addVariants?: string[]; removeVariants?: string[]; enabled?: boolean; meaning?: string }

function TermRow({ term, spellings, open, working, onToggle, onEdit, onDelete }: {
  term: DictionaryTerm
  spellings: string[]
  open: boolean
  working: boolean
  onToggle: () => void
  onEdit: (change: TermChange) => Promise<boolean>
  onDelete: () => void
}) {
  const words = W.termDictionary
  const [main, ...others] = spellings
  const enabled = term.enabled !== false
  const roman = term.roman?.trim() || term.sanskrit
  const mainButton = useRef<HTMLButtonElement>(null)
  return (
    <li
      className={enabled ? 'term-row' : 'term-row is-off'}
      // 펼친 줄 안에서 Esc: 접고 그 줄로 돌아간다(키보드만으로 쓸 때).
      onKeyDown={(event) => { if (open && event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onToggle(); mainButton.current?.focus() } }}
    >
      <button ref={mainButton} className="term-row-main" type="button" aria-expanded={open} onClick={onToggle}>
        <span className="term-head">
          <strong>{main ?? term.sanskrit}</strong>
          {roman && <span className="term-roman">{roman}</span>}
          {/* 뒤쪽이 주는 userKind 로 가른다(added = 내가 더함, modified = 내가 고침). */}
          {term.userKind === 'added' && <span className="term-mine">{words.mine}</span>}
          {term.userKind === 'modified' && <span className="term-mine">{words.edited}</span>}
          {!enabled && <span className="term-mine">{words.off}</span>}
        </span>
        {term.meaning && <span className="term-meaning">{term.meaning}</span>}
        {!open && others.length > 0 && (
          <span className="term-spellings" aria-label={words.spellings}>
            {others.map((text) => <span key={text} className="term-spelling">{text}</span>)}
          </span>
        )}
      </button>
      {open && <TermEditor term={term} spellings={spellings} working={working} onEdit={onEdit} onDelete={onDelete} />}
    </li>
  )
}

function TermEditor({ term, spellings, working, onEdit, onDelete }: { term: DictionaryTerm; spellings: string[]; working: boolean; onEdit: (change: TermChange) => Promise<boolean>; onDelete: () => void }) {
  const words = W.termDictionary
  const [asking, setAsking] = useState(false)
  const deleteButton = useRef<HTMLButtonElement>(null)
  const [meaning, setMeaning] = useState(term.meaning)
  const [spelling, setSpelling] = useState('')
  const spellingInput = useRef<HTMLInputElement>(null)
  const meaningInput = useRef<HTMLInputElement>(null)
  const useSwitch = useRef<HTMLButtonElement>(null)
  // 고친 뒤 누른 단추가 사라지거나(표기 삭제) 잠기면(뜻 저장, 쓰기 켜고 끄기) 그 자리 가까운 칸으로 포커스를 옮긴다.
  const edit = async (change: TermChange, next: () => HTMLElement | null) => { const ok = await onEdit(change); focusIfLost(next); return ok }
  useEffect(() => { setMeaning(term.meaning) }, [term.meaning])
  const mine = mySpellings(term)
  const enabled = term.enabled !== false
  // 내가 더한 용어의 마지막 표기는 지우지 않는다(표기 없는 용어가 된다).
  const canRemove = (text: string) => mine.has(text) && spellings.length > 1

  return (
    <div className="term-editor">
      <div className="term-spellings" aria-label={words.spellings}>
        {spellings.map((text) => (
          <span key={text} className="term-spelling">
            {text}
            {canRemove(text) && (
              <button type="button" className="term-spelling-remove" aria-label={words.removeSpelling(text)} disabled={working} onClick={() => void edit({ removeVariants: [text] }, () => spellingInput.current)}>
                <X size={11} aria-hidden />
              </button>
            )}
          </span>
        ))}
      </div>
      <form className="term-inline" onSubmit={async (event) => { event.preventDefault(); const value = spelling.trim(); if (value && await edit({ addVariants: [value] }, () => spellingInput.current)) setSpelling('') }}>
        <input ref={spellingInput} value={spelling} placeholder={words.addSpellingHint} aria-label={words.addSpellingHint} maxLength={100} onChange={(event) => setSpelling(event.target.value)} />
        <button className="secondary-button" type="submit" disabled={working || !spelling.trim()}>{words.addSpelling}</button>
      </form>
      <form className="term-inline" onSubmit={(event) => { event.preventDefault(); void edit({ meaning: meaning.trim() }, () => meaningInput.current) }}>
        <input ref={meaningInput} value={meaning} placeholder={words.fieldMeaning} aria-label={words.fieldMeaning} maxLength={500} onChange={(event) => setMeaning(event.target.value)} />
        <button className="secondary-button" type="submit" disabled={working || meaning.trim() === term.meaning.trim()}>{words.saveMeaning}</button>
      </form>
      <div className="term-use">
        <span>{words.useTerm}</span>
        <button
          ref={useSwitch}
          className={enabled ? 'skills-switch is-on' : 'skills-switch'}
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-label={words.useTerm}
          disabled={working}
          onClick={() => void edit({ enabled: !enabled }, () => useSwitch.current)}
        />
      </div>
      {/* 내가 더한 용어만 지울 수 있다(canDelete). 기본 용어는 끄기만 된다. */}
      {term.canDelete && (
        <div className="term-form-actions">
          <button ref={deleteButton} className="secondary-button" type="button" disabled={working} onClick={() => setAsking(true)}>{words.deleteTerm}</button>
        </div>
      )}
      {asking && (
        <QuestionDialog
          title={words.removeTermTitle(spellings[0] ?? term.sanskrit)}
          text={words.removeTermText}
          working={working}
          actions={[
            { label: words.cancel, onSelect: () => { setAsking(false); focusIfLost(() => deleteButton.current) }, autoFocus: true },
            { label: words.remove, primary: true, onSelect: () => { setAsking(false); onDelete() } },
          ]}
          onDismiss={() => { setAsking(false); focusIfLost(() => deleteButton.current) }}
        />
      )}
    </div>
  )
}

type PersonDraft = { name: string; aliases: string[]; confirmedByUser: boolean }

function PeopleList({ people, total, searching, adding, working, onStopAdding, onFocusAdd, onSave, onDelete }: {
  people: DictionaryPerson[]
  total: number
  searching: boolean
  adding: boolean
  working: boolean
  onStopAdding: () => void
  onFocusAdd: () => void
  onSave: (person: DictionaryPerson | null, next: PersonDraft) => Promise<boolean>
  onDelete: (person: DictionaryPerson) => Promise<boolean>
}) {
  const words = W.termDictionary
  const [editing, setEditing] = useState<string | null>(null)
  const [asking, setAsking] = useState<DictionaryPerson | null>(null)
  const keyOf = (person: DictionaryPerson, index: number) => person.id ?? `row-${index}`
  const list = useRef<HTMLUListElement>(null)
  // 그 이름 줄의 "수정" 단추로(확인됨으로 바꾼 뒤, 수정 창을 닫은 뒤).
  const focusRow = (key: string, which = 'term-person-edit') => focusIfLost(() => list.current?.querySelector<HTMLElement>(`[data-person-key="${CSS.escape(key)}"] .${which}`))
  // 묻는 창을 그만두거나 Esc 로 닫으면 그 줄의 "삭제" 단추로 돌아간다.
  const stopAsking = () => { if (asking) { const index = people.indexOf(asking); focusRow(keyOf(asking, index), 'term-person-delete') } setAsking(null) }

  return (
    <>
      {adding && (
        <PersonForm
          working={working}
          onCancel={onStopAdding}
          // 선생님이 직접 적은 이름은 사람이 확인한 이름이다.
          onSave={async (name, aliases) => { if (await onSave(null, { name, aliases, confirmedByUser: true })) onStopAdding() }}
        />
      )}
      <p className="term-count" role="status">{searching ? words.peopleFound(people.length) : words.peopleTotal(total)}</p>
      {people.length === 0 && <p className="term-note">{searching ? words.empty : words.peopleEmpty}</p>}
      {people.length > 0 && (
        <ul ref={list} className="term-rows">
          {people.map((person, index) => {
            const key = keyOf(person, index)
            if (editing === key) {
              return (
                <li key={key} className="term-row">
                  <PersonForm
                    person={person}
                    working={working}
                    onCancel={() => { setEditing(null); focusRow(key) }}
                    // 수정은 확인 상태를 바꾸지 않는다. 확인은 "확인됨으로 바꾸기"로만.
                    onSave={async (name, aliases) => { if (await onSave(person, { name, aliases, confirmedByUser: person.confirmed })) { setEditing(null); focusRow(person.id ?? key) } }}
                  />
                </li>
              )
            }
            return (
              <li key={key} className="term-row term-person" data-person-key={key}>
                <div className="term-person-main">
                  <span className="term-head">
                    <strong>{person.name}</strong>
                    {!person.confirmed && <span className="term-mine">{words.unconfirmed}</span>}
                  </span>
                  {person.aliases.length > 0 && (
                    <span className="term-spellings" aria-label={words.aliases}>
                      {person.aliases.map((text) => <span key={text} className="term-spelling">{text}</span>)}
                    </span>
                  )}
                </div>
                <div className="term-person-actions">
                  {!person.confirmed && (
                    <button className="secondary-button" type="button" disabled={working} onClick={async () => { await onSave(person, { name: person.name, aliases: person.aliases, confirmedByUser: true }); focusRow(key) }}>{words.confirm}</button>
                  )}
                  <button className="secondary-button term-person-edit" type="button" disabled={working} onClick={() => setEditing(key)}>{words.edit}</button>
                  {person.id && <button className="secondary-button term-person-delete" type="button" disabled={working} onClick={() => setAsking(person)}>{words.remove}</button>}
                </div>
              </li>
            )
          })}
        </ul>
      )}
      {asking && (
        <QuestionDialog
          title={words.removePersonTitle(asking.name)}
          text={words.removePersonText}
          working={working}
          actions={[
            { label: words.cancel, onSelect: stopAsking, autoFocus: true },
            { label: words.remove, primary: true, onSelect: async () => { const target = asking; setAsking(null); await onDelete(target); onFocusAdd() } },
          ]}
          onDismiss={stopAsking}
        />
      )}
    </>
  )
}

function PersonForm({ person, working, onCancel, onSave }: { person?: DictionaryPerson; working: boolean; onCancel: () => void; onSave: (name: string, aliases: string[]) => void }) {
  const words = W.termDictionary
  const [name, setName] = useState(person?.name ?? '')
  const [aliases, setAliases] = useState((person?.aliases ?? []).join(', '))
  const list = aliases.split(/[,，]/).map((text) => text.trim()).filter(Boolean)
  return (
    <form className="term-form" onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel() } }} onSubmit={(event) => { event.preventDefault(); if (name.trim()) onSave(name.trim(), [...new Set(list)]) }}>
      <label className="setting-field">
        <span>{words.fieldName}</span>
        <input value={name} onChange={(event) => setName(event.target.value)} maxLength={100} autoFocus />
      </label>
      <label className="setting-field">
        <span>{words.fieldAliases}</span>
        <input value={aliases} onChange={(event) => setAliases(event.target.value)} />
      </label>
      <div className="term-form-actions">
        <button className="secondary-button" type="button" onClick={onCancel}>{words.cancel}</button>
        <button className="confirm-button" type="submit" disabled={working || !name.trim()}>{words.save}</button>
      </div>
    </form>
  )
}
