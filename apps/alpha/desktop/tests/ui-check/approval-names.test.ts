// 승인 카드의 폴더 이름·원자료 이름(src/approval-names.ts, 2026-11-19 G82 folder·originalFileName)의 단위 시험.
// 실행: npx tsx --test tests/ui-check/approval-names.test.ts
import assert from 'node:assert/strict'
import test from 'node:test'
import { approvalFolderName, targetName } from '../../src/approval-names'
import { W } from '../../src/words'

test('폴더 값이 있으면 그 폴더 이름(마지막 이름)', () => {
  assert.equal(approvalFolderName({ folder: 'class/notes' }), 'notes')
  assert.equal(approvalFolderName({ folder: 'class' }), 'class')
})

test('폴더 "" 는 맨 위: 문서 화면의 맨 위 이름', () => {
  assert.equal(approvalFolderName({ folder: '' }), W.document)
})

test('폴더 null·값 없음이면 적지 않는다', () => {
  assert.equal(approvalFolderName({ folder: null }), null)
  assert.equal(approvalFolderName({}), null)
})

test('원자료 경로에 원래 이름이 있으면 그 이름', () => {
  assert.equal(targetName('objects/sha256/ab/cd/abcdef', 'lesson.png'), 'lesson.png')
  assert.equal(targetName('C:\\archive\\objects\\sha256\\ab\\abcdef', '수업 사진.jpg'), '수업 사진.jpg')
})

test('원자료 경로에 원래 이름이 null·빈 글이면 "원자료 파일"', () => {
  assert.equal(targetName('objects/sha256/ab/cd/abcdef', null), W.approvalCard.sourceFile)
  assert.equal(targetName('objects/sha256/ab/cd/abcdef', ''), W.approvalCard.sourceFile)
  assert.equal(targetName('objects/sha256/ab/cd/abcdef'), W.approvalCard.sourceFile)
})

test('원자료가 아닌 경로는 원래 이름이 와도 파일 이름 그대로', () => {
  assert.equal(targetName('zettels/zet_1.md', 'lesson.png'), 'zet_1.md')
})
