import assert from 'node:assert/strict'
import test from 'node:test'
import {
  assetKindForFileName,
  collectSourceText,
  describeImportPlan,
  fileNameFromPath,
  isSupportedSourceName,
  mediaTypeForFileName,
  summaryFromParagraphs,
  transcriptFromText,
} from '../src/source-import'
import type { SourceAssetImportPlan, SourceTextPage } from '../src/runtime'

test('Windows 경로에서 파일 이름과 종류를 읽는다', () => {
  assert.equal(fileNameFromPath('C:\\Users\\yoga\\Recordings\\example-class.m4a'), 'example-class.m4a')
  assert.equal(fileNameFromPath('/home/yoga/수업 전사.TXT'), '수업 전사.TXT')
  assert.equal(assetKindForFileName('수업.m4a'), 'audio_recording')
  assert.equal(assetKindForFileName('수업 전사.TXT'), 'raw_transcript')
  assert.equal(assetKindForFileName('whisper.json'), 'raw_transcript')
  assert.equal(assetKindForFileName('교재.pdf'), 'other')
  assert.equal(mediaTypeForFileName('a.m4a'), 'audio/mp4')
  assert.equal(mediaTypeForFileName('a.md'), 'text/markdown')
  assert.equal(mediaTypeForFileName('a.pdf'), null)
  assert.equal(isSupportedSourceName('a.flac'), true)
  assert.equal(isSupportedSourceName('a.pdf'), false)
})

test('가져오기 미리보기는 중복 보존을 구분해 말한다', () => {
  const plan: SourceAssetImportPlan = {
    planDigest: 'd'.repeat(64),
    originalFileName: '수업.m4a',
    assetKind: 'audio_recording',
    contentSha256: 'abcdef0123456789'.repeat(4),
    byteLength: 3 * 1024 * 1024,
    alreadyStored: false,
    recordLink: null,
    requiresConfirmation: true,
  }
  assert.equal(describeImportPlan(plan), '3.0 MB · SHA-256 · abcdef01…23456789 · 원자료를 복사해 보존합니다')
  assert.match(describeImportPlan({ ...plan, alreadyStored: true }), /이미 보존된 원자료와 같은 내용/)
})

test('원문 페이지는 첫 페이지 해시로 묶어 끝까지 이어 붙인다', async () => {
  const sha = 'f'.repeat(64)
  const pages: SourceTextPage[] = [
    { assetId: 'a', contentSha256: sha, byteLength: 12, cursor: 0, nextCursor: 6, content: '첫 줄\n' },
    { assetId: 'a', contentSha256: sha, byteLength: 12, cursor: 6, nextCursor: null, content: '둘째 줄' },
  ]
  const seen: Array<[number, string | undefined]> = []
  const result = await collectSourceText(async (cursor, expected) => {
    seen.push([cursor, expected])
    return pages.find((page) => page.cursor === cursor)!
  })
  assert.equal(result.text, '첫 줄\n둘째 줄')
  assert.equal(result.contentSha256, sha)
  assert.deepEqual(seen, [[0, undefined], [6, sha]])
})

test('본문 상한을 넘는 원문은 본문에 넣지 않는다', async () => {
  await assert.rejects(
    collectSourceText(async () => ({
      assetId: 'a', contentSha256: 'e'.repeat(64), byteLength: 5 * 1024 * 1024, cursor: 0, nextCursor: null, content: '',
    })),
    /원자료만 보존/,
  )
})

test('원문 문단은 전사 줄과 요약 절로 그대로 옮겨진다', () => {
  const { transcript, paragraphs } = transcriptFromText('첫 문단\r\n\r\n둘째 문단\n  \n셋째 문단\n넷째 문단')
  assert.equal(paragraphs.length, 4)
  assert.deepEqual(transcript[1], { time: '문단 2', speaker: '원문', text: '둘째 문단' })
  const sections = summaryFromParagraphs(paragraphs, 'fallback', ' 메모 ')
  assert.equal(sections[0].source, '문단 1–3')
  assert.equal(sections[0].body, '첫 문단\n\n둘째 문단\n\n셋째 문단')
  assert.deepEqual(sections[1], { title: '기록 메모', body: '메모', source: '가져오기 메모' })
  assert.equal(summaryFromParagraphs([], 'fallback', '').length, 1)
})
