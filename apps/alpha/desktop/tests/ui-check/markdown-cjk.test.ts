// 한국어 강조 고치기(src/markdown-cjk.ts)의 단위 시험. 화면과 같은 react-markdown·remark-gfm 으로 그려 본다.
// 실행: npx tsx --test tests/ui-check/markdown-cjk.test.ts
import assert from 'node:assert/strict'
import test from 'node:test'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { closeOpenBold, cjkFriendlyMarkdown, remarkStripZeroWidth } from '../../src/markdown-cjk'

function html(text: string) {
  return renderToStaticMarkup(createElement(Markdown, { remarkPlugins: [remarkGfm, remarkStripZeroWidth] }, cjkFriendlyMarkdown(text))).replace(/&#x27;/g, "'").replace(/&quot;/g, '"')
}

test('조사 앞 문장 부호로 끝나는 굵게·기울임·취소선이 그려진다', () => {
  const cases: Array<[string, string]> = [
    ["이것은 **'중요'**입니다", "<strong>'중요'</strong>입니다"],
    ['**설명(괄호)**을 봅니다', '<strong>설명(괄호)</strong>을'],
    ['**"인용"**이 있습니다', '<strong>"인용"</strong>이'],
    ['호흡을 **천천히.**그다음', '<strong>천천히.</strong>그다음'],
    ['**아사나(asana)**는 자세다', '<strong>아사나(asana)</strong>는'],
    ['**50%**가 남았다', '<strong>50%</strong>가'],
    ['*기울임(예)*다', '<em>기울임(예)</em>다'],
    ['~~취소(예)~~다', '<del>취소(예)</del>다'],
    ['자세는**"고양이"**와 같다', '자세는<strong>"고양이"</strong>와'],
    ['***굵은 기울임(예)***다', '<em><strong>굵은 기울임(예)</strong></em>다'],
  ]
  for (const [text, expected] of cases) {
    const out = html(text)
    assert.ok(out.includes(expected), `${text} → ${out}`)
    assert.ok(!out.includes('**') && !out.includes('​'), `${text} → ${out}`)
  }
})

test('원래 잘 되던 것은 그대로다', () => {
  assert.equal(html('**굵게**다'), '<p><strong>굵게</strong>다</p>')
  assert.equal(html('**굵게** 다'), '<p><strong>굵게</strong> 다</p>')
  assert.match(html('- **항목**: 설명'), /<li><strong>항목<\/strong>: 설명<\/li>/)
  assert.match(html('## **제목**이다'), /<h2><strong>제목<\/strong>이다<\/h2>/)
  assert.equal(html('`코드`다'), '<p><code>코드</code>다</p>')
  assert.equal(html('[링크](https://x.com)다'), '<p><a href="https://x.com">링크</a>다</p>')
})

test('코드와 이스케이프는 건드리지 않는다', () => {
  assert.equal(html("`**'a'**b`"), "<p><code>**'a'**b</code></p>")
  assert.match(html("```\n**'a'**b\n```"), /<code>\*\*'a'\*\*b\n<\/code>/)
  assert.equal(html('별표 \\*그대로(예)\\*다'), '<p>별표 *그대로(예)*다</p>')
})

test('답이 오는 중이면 닫히지 않은 ** 를 닫아 굵게 그린다', () => {
  assert.equal(closeOpenBold('호흡을 **천천'), '호흡을 **천천**')
  assert.equal(closeOpenBold('**다 닫힘**'), '**다 닫힘**')
  assert.equal(closeOpenBold('`**` 코드 안'), '`**` 코드 안')
  assert.ok(html(closeOpenBold("이것은 **'중요")).includes("<strong>'중요</strong>"))
})
