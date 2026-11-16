import { useDeferredValue, useEffect, useMemo, useState } from 'react'
import { Search } from 'lucide-react'
import { listYogaTerms, normalizeQuery, searchText, shownSpellings, type DictionaryTerm } from '../dictionary'
import { archiveErrorMessage } from '../archive'
import { W } from '../words'

/**
 * 설정 › 용어 사전(가장 작게 한 벌, 보기만. 그가 보고 고친다). 2026-11-16 그의 결정: 이름은 "용어 사전", 여러 표기를 다 적어 알아보는 데 쓴다.
 * 위에 찾기 칸 하나, 아래에 갈래(자세, 호흡 등)마다 용어 줄. 한 줄 = 대표 표기(한글), 산스크리트 로마자, 뜻 한 줄, 다른 표기들, 내가 더한 것이면 그 표시.
 * 찾기는 한글 표기·산스크리트 로마자·우리말 이름·다른 표기·뜻 모두에서, 띄어쓰기와 로마자의 점 같은 표시는 무시한다.
 * 더하기·고치기 단추는 없다(쓰기 명령이 아직 없다). 출처 주소와 "사람 검수 전" 같은 사전의 상태는 보이지 않는다.
 */
export function TermDictionarySettings() {
  const [terms, setTerms] = useState<DictionaryTerm[] | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const deferred = useDeferredValue(query)
  const words = W.termDictionary

  useEffect(() => {
    let alive = true
    listYogaTerms().then((list) => { if (alive) setTerms(list.terms) }).catch((error) => { if (alive) setProblem(archiveErrorMessage(error, words.failed)) })
    return () => { alive = false }
  }, [words.failed])

  // 찾을 글은 한 번만 만든다(수백 개라도 적을 때마다 다시 만들지 않는다).
  const indexed = useMemo(() => (terms ?? []).map((term) => ({ term, text: searchText(term), spellings: shownSpellings(term) })), [terms])
  const groups = useMemo(() => {
    const needle = normalizeQuery(deferred)
    const found = needle ? indexed.filter((item) => item.text.includes(needle)) : indexed
    const byCategory = new Map<string, typeof found>()
    for (const item of found) {
      const key = item.term.category?.trim() || words.otherCategory
      byCategory.set(key, [...(byCategory.get(key) ?? []), item])
    }
    return { count: found.length, list: [...byCategory.entries()] }
  }, [indexed, deferred, words.otherCategory])

  return (
    <div className="settings-page term-dictionary">
      <header className="settings-page-heading">
        <h2>{words.title}</h2>
        <p>{words.intro}</p>
      </header>
      <label className="term-search">
        <Search size={15} aria-hidden />
        <input value={query} placeholder={words.searchHint} aria-label={words.searchHint} spellCheck={false} onChange={(event) => setQuery(event.target.value)} />
      </label>
      {problem ? (
        <p className="term-note" role="alert">{problem}</p>
      ) : terms === null ? (
        <p className="term-note" role="status">{words.loading}</p>
      ) : (
        <>
          <p className="term-count" role="status">{query.trim() ? words.found(groups.count) : words.total(terms.length)}</p>
          {groups.count === 0 && <p className="term-note">{words.empty}</p>}
          {groups.list.map(([category, items]) => (
            <section key={category} className="term-group" aria-label={category}>
              <h3>{category}</h3>
              <ul className="term-rows">
                {items.map(({ term, spellings }) => {
                  const [main, ...others] = spellings
                  return (
                    <li key={term.id} className="term-row">
                      <div className="term-head">
                        <strong>{main ?? term.sanskrit}</strong>
                        <span className="term-roman">{term.roman?.trim() || term.sanskrit}</span>
                        {term.origin === 'user' && <span className="term-mine">{words.mine}</span>}
                      </div>
                      {term.meaning && <p className="term-meaning">{term.meaning}</p>}
                      {others.length > 0 && (
                        <p className="term-spellings" aria-label={words.spellings}>
                          {others.map((text) => <span key={text} className="term-spelling">{text}</span>)}
                        </p>
                      )}
                    </li>
                  )
                })}
              </ul>
            </section>
          ))}
        </>
      )}
    </div>
  )
}
