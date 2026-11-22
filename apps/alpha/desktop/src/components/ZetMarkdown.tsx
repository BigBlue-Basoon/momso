import { Fragment, isValidElement, memo, type CSSProperties, type ReactNode } from 'react'
import Markdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { findZetId, isWebAddress } from '../archive'
import { closeOpenBold, cjkFriendlyMarkdown, remarkStripZeroWidth } from '../markdown-cjk'
import { W } from '../words'

type MarkdownNode = {
  type: string
  value?: string
  children?: MarkdownNode[]
}

// 글 안의 줄바꿈을 화면에서도 줄바꿈으로 보여 준다(문단 안의 한 줄 바꿈).
function splitSoftBreaks(node: MarkdownNode) {
  if (!node.children) return
  const next: MarkdownNode[] = []
  for (const child of node.children) {
    if (child.type === 'text' && typeof child.value === 'string' && child.value.includes('\n')) {
      child.value.split(/\r?\n/).forEach((part, index) => {
        if (index > 0) next.push({ type: 'break' })
        if (part) next.push({ type: 'text', value: part })
      })
    } else {
      splitSoftBreaks(child)
      next.push(child)
    }
  }
  node.children = next
}

function remarkSoftBreaks() {
  return (tree: MarkdownNode) => splitSoftBreaks(tree)
}

type TableNode = {
  tagName?: string
  children?: unknown[]
}

function childElements(node: TableNode | undefined) {
  return (node?.children ?? []).filter(
    (child): child is TableNode => typeof child === 'object' && child !== null && 'tagName' in child,
  )
}

function countColumns(table: TableNode | undefined) {
  const section = childElements(table).find((child) => child.tagName === 'thead' || child.tagName === 'tbody')
  const row = childElements(section).find((child) => child.tagName === 'tr')
  return childElements(row).filter((child) => child.tagName === 'th' || child.tagName === 'td').length || 1
}

type ZetMarkdownProps = {
  text: string
  /** 본문 안에서 다른 글을 가리키는 링크를 눌렀을 때. */
  onOpenZet: (id: string) => void
  /** 본문 안의 바깥 주소(http, https)를 눌렀을 때. */
  onOpenAddress: (address: string) => void
  /** 답이 오는 중이면 닫는 ** 가 아직 안 와도 굵게 그린다. */
  streaming?: boolean
}

// 굵은 글 화자: 추정이면 (시각 )?물음표+이름, 확인된 화자는 시각+이름(`**00:00 강사**`).
// 줄 맨 앞 화자: (물음표)?이름 다음 쌍점(':' 또는 '：').
const STRONG_SPEAKER = /^(\d{1,2}:\d{2}(?::\d{2})?\s+)?(\?)?([^\s?][^?]{0,19})$/
const LINE_SPEAKER = /^(\?)?([^\s:：?][^:：?]{0,19}?)(\s*[:：])/
// 문서 전체에서 줄 맨 앞 "이름:" 을 센다. 사람 이름이 아닌 것(화자를 가리지 못한 말).
const LINE_LABEL = /^[ \t]*(\?)?([^\s:：?*#>|`\-][^:：?\n]{0,19}?)[ \t]*[:：][ \t]/gm
const NOT_PERSON = new Set(['화자 불명'])

/**
 * 이 문서에서 줄 맨 앞 "이름:" 을 화자로 볼 이름들. 추정 표시(`?이름`)가 한 번이라도 있거나, 같은 이름이 두 줄 이상에서
 * 말한 사람으로 나오면 화자다(한 번뿐인 "참고:" 같은 머리말을 사람 이름으로 그리지 않으려고).
 */
function findSpeakers(text: string) {
  const counts = new Map<string, number>()
  const guessed = new Set<string>()
  for (const match of text.matchAll(LINE_LABEL)) {
    const name = match[2].trim()
    if (!name || NOT_PERSON.has(name)) continue
    if (match[1]) guessed.add(name)
    counts.set(name, (counts.get(name) ?? 0) + 1)
  }
  return new Set([...counts].filter(([name, count]) => guessed.has(name) || count >= 2).map(([name]) => name))
}

function plainText(children: ReactNode): string {
  if (typeof children === 'string' || typeof children === 'number') return String(children)
  if (Array.isArray(children)) return children.map(plainText).join('')
  return ''
}

/**
 * 사람 이름(문서 본문의 말한 사람). 2026-11-16 김성균 개발자가 정함: 추정이든 아니든 사람 이름에는 언제나 회색 바탕을 이름 뒤에 깐다.
 * 추정한 화자는 바탕 안 이름 앞에 작은 "?"(올리면 "추정한 화자")를 남긴다.
 */
function PersonName({ name, guessed }: { name: string; guessed: boolean }) {
  return (
    <span className={guessed ? 'zet-speaker is-guessed' : 'zet-speaker'}>
      {guessed && <span className="zet-speaker-mark" title={W.zetMarkdown.guessedSpeaker} aria-label={W.zetMarkdown.guessedSpeaker}>?</span>}
      {name}
    </span>
  )
}

/** 문단 안의 줄(맨 앞 글과 줄바꿈 바로 뒤의 글)마다, 맨 앞 "이름:" 이 화자면 이름에 바탕을 깐다. */
function markLineSpeakers(children: ReactNode, speakers: Set<string>) {
  const list = Array.isArray(children) ? children : [children]
  let changed = false
  const next = list.map((child, index) => {
    const lineStart = index === 0 || (isValidElement(list[index - 1]) && (list[index - 1] as { type?: unknown }).type === 'br')
    if (!lineStart || typeof child !== 'string') return child
    const match = LINE_SPEAKER.exec(child)
    if (!match) return child
    const name = match[2].trim()
    if (NOT_PERSON.has(name) || !(match[1] || speakers.has(name))) return child
    changed = true
    return (
      <Fragment key={`speaker-${index}`}>
        <PersonName name={name} guessed={Boolean(match[1])} />
        {child.slice(match[0].length - match[3].length)}
      </Fragment>
    )
  })
  return changed ? next : null
}

function buildComponents(onOpenZet: (id: string) => void, onOpenAddress: (address: string) => void, speakers: Set<string>): Components {
  return {
    // 글 제목이 이미 맨 위에 있으므로 본문의 제목은 한 단계씩 낮춰 그린다.
    h1: ({ children }) => <h2>{children}</h2>,
    h2: ({ children }) => <h3>{children}</h3>,
    h3: ({ children }) => <h4>{children}</h4>,
    h4: ({ children }) => <h5>{children}</h5>,
    h5: ({ children }) => <h6>{children}</h6>,
    a: ({ href, children }) => (
      <ZetLink href={href} onOpenZet={onOpenZet} onOpenAddress={onOpenAddress}>{children}</ZetLink>
    ),
    // 화자 표시(뒤쪽 규칙 archive_rules.md): AI 가 스스로 가린 화자는 이름 앞에 `?`(예: `?김성균`), 사람이 확인하면 `?` 가 없어진다.
    // 굵은 글의 화자(`**00:00 강사**`, `**00:00 ?강사**`)와 줄 맨 앞의 `이름:`, `?이름:` 모두 이름에 회색 바탕(PersonName).
    // 시각만 있는 굵은 글이 아닌 것(시각 없이 물음표도 없는 굵은 글)은 사람 이름으로 보지 않는다.
    strong: ({ children }) => {
      const match = STRONG_SPEAKER.exec(plainText(children))
      if (!match || (!match[1] && !match[2]) || NOT_PERSON.has(match[3].trim())) return <strong>{children}</strong>
      return (
        <strong className="zet-speaker-line">
          {match[1] && <span className="zet-speaker-time">{match[1].trim()} </span>}
          <PersonName name={match[3]} guessed={Boolean(match[2])} />
        </strong>
      )
    },
    p: ({ children }) => {
      const marked = markLineSpeakers(children, speakers)
      return <p>{marked ?? children}</p>
    },
    // 그림은 불러오지 않는다. 그림에 붙은 설명 글만 보여 준다.
    img: ({ alt }) => (alt ? <span className="zet-md-image">{alt}</span> : null),
    // 칸이 많은 표는 칸이 찌그러지지 않게 표만 옆으로 밀 수 있게 한다.
    table: ({ children, node }) => (
      <div className="zet-md-table">
        <table style={{ '--zet-table-columns': countColumns(node) } as CSSProperties}>{children}</table>
      </div>
    ),
  }
}

function ZetLink({
  href,
  children,
  onOpenZet,
  onOpenAddress,
}: {
  href: string | undefined
  children: ReactNode
  onOpenZet: (id: string) => void
  onOpenAddress: (address: string) => void
}) {
  const zetId = href ? findZetId(href) : null
  if (zetId) {
    return (
      <button className="zet-md-link" type="button" onClick={() => onOpenZet(zetId)}>
        {children}
      </button>
    )
  }
  // 바깥 주소는 기본 브라우저에서 연다. 앱 화면이 그 주소로 넘어가지 않도록 a 태그를 쓰지 않는다.
  if (href && isWebAddress(href)) {
    return (
      <button className="zet-md-link" type="button" role="link" title={href} onClick={() => onOpenAddress(href)}>
        {children}
      </button>
    )
  }
  // 그 밖의 주소는 누를 수 없는 글자로 둔다.
  return <span className="zet-md-address" title={href}>{children}</span>
}

/**
 * 글 본문(마크다운)을 그린다. 본문 안의 HTML 은 실행하지 않고 글자로 보여 준다.
 */
export const ZetMarkdown = memo(function ZetMarkdown({ text, onOpenZet, onOpenAddress, streaming = false }: ZetMarkdownProps) {
  // 한국어 조사 앞에서 **…** 가 깨지지 않게 맞춘 뒤 그린다(markdown-cjk.ts, 2026-11-18).
  const source = cjkFriendlyMarkdown(streaming ? closeOpenBold(text) : text)
  return (
    <div className="zet-md">
      <Markdown remarkPlugins={[remarkGfm, remarkSoftBreaks, remarkStripZeroWidth]} components={buildComponents(onOpenZet, onOpenAddress, findSpeakers(text))}>
        {source}
      </Markdown>
    </div>
  )
})
