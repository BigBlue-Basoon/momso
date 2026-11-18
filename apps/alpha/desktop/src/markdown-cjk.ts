// 한국어에서 굵게·기울임·취소선이 깨지는 것 고치기(2026-11-18 그의 말 "AI답변에서 ** 이렇게 마크다운 깨지는게 보이고").
//
// 까닭: CommonMark 의 강조 규칙(flanking)은 띄어쓰기를 낱말 사이에 두는 글을 생각해 만들어졌다.
//   닫는 ** 바로 앞이 문장 부호(' " ) . % 등)이고 바로 뒤가 글자(조사 '은·는·이·가·을' 등)이면 닫는 것으로 보지 않는다.
//   예: **'중요'**입니다, **아사나(asana)**는, **50%**가 → ** 가 그대로 보인다. 여는 쪽도 거꾸로(글자**'…) 같다.
// 고침: 그런 자리에만 보이지 않는 글자(U+200B, 문장 부호도 띄어쓰기도 아님)를 끼워 규칙을 맞춘 뒤 그리고,
//   그린 글에서는 그 글자를 다시 뺀다(remarkStripZeroWidth). 코드(``` 묶음, `…`)와 \* 같은 이스케이프는 건드리지 않는다.
const ZWSP = '​'
// 문장 부호·기호. 강조 표시 자체(* ~ _), 코드(`), 이스케이프(\)는 뺀다(그것들 사이에 끼우면 뜻이 바뀐다).
const PUNCT = String.raw`(?![*~_\x60\\])[\p{P}\p{S}]`
const WORD = String.raw`[\p{L}\p{N}]`
const DELIM = String.raw`\*{1,3}|~~`
const CLOSING = new RegExp(`(${PUNCT})(${DELIM})(?=${WORD})`, 'gu')
const OPENING = new RegExp(`(?<=${WORD})(${DELIM})(${PUNCT})`, 'gu')

function fixLine(text: string) {
  return text.replace(CLOSING, `$1${ZWSP}$2`).replace(OPENING, `$1${ZWSP}$2`)
}

/** 코드가 아닌 곳에만 f 를 쓴다. ``` 또는 ~~~ 로 묶인 줄과 줄 안의 `…` 는 그대로 둔다. */
function outsideCode(text: string, f: (part: string) => string) {
  const lines = text.split('\n')
  let fence: string | null = null
  return lines.map((line) => {
    const open = /^\s{0,3}(`{3,}|~{3,})/.exec(line)
    if (fence) {
      if (open && open[1][0] === fence[0] && open[1].length >= fence.length && !line.trim().slice(open[1].length).trim()) fence = null
      return line
    }
    if (open) { fence = open[1]; return line }
    // 줄 안의 코드(`…`, ``…``)는 건너뛴다.
    return line.split(/(`+[^`]*?`+)/).map((part, index) => (index % 2 === 1 ? part : f(part))).join('')
  }).join('\n')
}

/** 그리기 전에 한국어 강조 자리를 맞춘다. */
export function cjkFriendlyMarkdown(text: string) {
  if (!text.includes('*') && !text.includes('~~')) return text
  return outsideCode(text, fixLine)
}

/** 답이 오는 중이라 닫는 ** 가 아직 안 왔으면 끝에 붙여 굵게 그린다(** 가 잠깐 보이지 않게). */
export function closeOpenBold(text: string) {
  let count = 0
  outsideCode(text, (part) => { count += (part.match(/\*\*/g) ?? []).length; return part })
  return count % 2 === 1 ? `${text}**` : text
}

type TextNode = { type: string; value?: string; children?: TextNode[] }
function strip(node: TextNode) {
  if (node.type === 'text' && typeof node.value === 'string' && node.value.includes(ZWSP)) node.value = node.value.split(ZWSP).join('')
  node.children?.forEach(strip)
}
/** cjkFriendlyMarkdown 이 끼운 보이지 않는 글자를 그린 글에서 뺀다(복사해도 섞이지 않게). */
export function remarkStripZeroWidth() {
  return (tree: TextNode) => strip(tree)
}
