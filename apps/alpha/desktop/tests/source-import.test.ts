import assert from 'node:assert/strict'
import test from 'node:test'
import {
  assetKindForFileName,
  fileNameFromPath,
  isSupportedSourceName,
  mediaTypeForFileName,
} from '../src/source-import'

// 2026-11-16 옛 "새 기록" 창을 없애며 그 창만 쓰던 것(가져오기 미리보기 글, 원문 페이지 잇기, 전사 줄·요약 절 만들기)의 시험은 뺐다.
test('Windows 경로에서 파일 이름과 종류를 읽는다', () => {
  assert.equal(fileNameFromPath(['C:', 'Users', 'yoga', 'Recordings', 'example-class.m4a'].join('\\')), 'example-class.m4a')
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
