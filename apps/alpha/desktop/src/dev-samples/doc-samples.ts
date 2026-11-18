// 개발 앱에서만 보이는 문서와 폴더(2026-11-18 김성균 개발자 요청: 문서 화면을 폴더 꼴로 보고 피드백하려면 무언가 보여야 함).
// archive.ts 가 import.meta.env.DEV 일 때만 불러온다. 설치판(vite build)에는 들어가지 않는다.
// 읽기와 고치기(저장)는 이 실행 동안 메모리에서만 된다. 사람 이름은 넣지 않았다.
import type { ZetDocument, ZetHistoryEntry, ZetLinked, ZetListEntry, ZetKind, ZetStatus } from '../archive'

type Sample = { id: string; folder: string; title: string; summary: string; kind: ZetKind; status: ZetStatus; day: number; body: string }

const stamp = (day: number, hour = 21) => new Date(2026, 10, day, hour, 0).toISOString()

const SAMPLES: Sample[] = [
  { id: 'dev-doc-class-1117', folder: '수업 기록/2026-11', title: '11월 17일 아침 빈야사', summary: '태양 경배 B 를 천천히 세 번, 전사 자세 흐름을 새로 넣었다.', kind: 'summary', status: 'confirmed', day: 17,
    body: '## 흐름\n\n1. 호흡 고르기 3분\n2. 태양 경배 A 두 번, B 세 번\n3. 전사 1 → 전사 2 → 리버스 전사\n\n## 수련생 반응\n\n- 처음 온 수련생 둘이 전사 2 에서 무릎이 안으로 모였다. 다음 시간에 벽을 써 보기.\n' },
  { id: 'dev-doc-class-1115', folder: '수업 기록/2026-11', title: '11월 15일 저녁 하타', summary: '호흡 속도를 낮추고 고관절을 차례로 열었다.', kind: 'summary', status: 'confirmed', day: 15,
    body: '## 흐름\n\n- 누운 자세에서 무릎 당기기\n- 나비 자세, 반 비둘기\n- 마무리 사바아사나 7분\n\n왼쪽 어깨가 걸린다는 수련생이 있어 팔 범위를 줄였다.\n' },
  { id: 'dev-doc-class-1115-t', folder: '수업 기록/2026-11', title: '11월 15일 저녁 하타 전사', summary: '수업 전체를 받아 적고 용어를 고쳤다.', kind: 'transcript', status: 'draft', day: 15,
    body: '자, 편하게 누워서 무릎을 가슴 쪽으로 당겨 볼게요. 숨을 내쉬면서 조금 더 당기고, 들이쉬면서 풀어 줍니다.\n\n이제 나비 자세로 앉아 볼게요. 발바닥을 맞대고…\n' },
  { id: 'dev-doc-class-1112', folder: '수업 기록/2026-11', title: '11월 12일 저녁 하타 돌아보기', summary: '처음 온 수련생이 많아 기본 자세 위주로 바꿨다.', kind: 'note', status: 'draft', day: 12,
    body: '처음 온 분이 다섯 명이라 계획한 흐름 대신 기본 서기 자세와 호흡만 했다.\n\n다음에는 첫 시간용 흐름을 따로 만들어 두기.\n' },
  { id: 'dev-doc-class-1029', folder: '수업 기록/2026-10', title: '10월 29일 아침 빈야사', summary: '팔 균형 자세를 처음 소개했다.', kind: 'summary', status: 'confirmed', day: 1,
    body: '까마귀 자세를 블록 위에서 연습했다. 손목 풀기를 충분히.\n' },
  { id: 'dev-doc-class-1022', folder: '수업 기록/2026-10', title: '10월 22일 저녁 하타', summary: '비가 와서 실내 온도를 높이고 천천히 했다.', kind: 'summary', status: 'confirmed', day: 1,
    body: '실내가 추워 시작 전 10분 동안 몸을 데웠다.\n' },
  { id: 'dev-doc-promo-dec', folder: '홍보 글', title: '12월 수업 안내 글', summary: '인스타그램과 블로그에 올릴 12월 시간표 안내.', kind: 'note', status: 'draft', day: 16,
    body: '## 12월 수업 안내\n\n- 월·수 저녁 7시 반 하타\n- 화·목 아침 7시 빈야사\n- 토요일 오전 10시 처음 오시는 분 수업\n\n첫 수업은 무료입니다. 매트는 빌려 드려요.\n' },
  { id: 'dev-doc-promo-beginner', folder: '홍보 글', title: '처음 오시는 분께', summary: '처음 오는 수련생에게 보낼 안내 문자.', kind: 'note', status: 'confirmed', day: 10,
    body: '수업 10분 전에 오셔서 편한 옷으로 갈아입어 주세요. 식사는 두 시간 전까지 가볍게 하시는 걸 권해 드려요.\n' },
  { id: 'dev-doc-promo-review', folder: '홍보 글', title: '11월 후기 모음', summary: '수련생들이 남긴 짧은 후기를 모았다.', kind: 'note', status: 'draft', day: 14,
    body: '- "어깨가 많이 가벼워졌어요."\n- "저녁 수업 뒤에 잠이 잘 와요."\n' },
  { id: 'dev-doc-term-poses', folder: '용어 사전', title: '자세 이름', summary: '수업에서 쓰는 자세 이름과 산스크리트어.', kind: 'note', status: 'confirmed', day: 8,
    body: '| 한국어 | 산스크리트어 |\n|---|---|\n| 나무 자세 | 브륵샤아사나 |\n| 전사 자세 2 | 비라바드라아사나 2 |\n| 아래를 보는 개 | 아도 무카 스바나아사나 |\n' },
  { id: 'dev-doc-term-breath', folder: '용어 사전', title: '호흡 이름', summary: '우짜이, 나디 쇼다나 등 호흡법 정리.', kind: 'note', status: 'confirmed', day: 8,
    body: '- 우짜이: 목 뒤를 살짝 좁혀 소리 나게 쉬는 호흡\n- 나디 쇼다나: 한쪽 콧구멍씩 번갈아 쉬는 호흡\n' },
  { id: 'dev-doc-member-mon', folder: '회원 메모', title: '월요일 저녁반', summary: '허리 통증이 있는 수련생 둘, 앞으로 숙이기 조심.', kind: 'note', status: 'draft', day: 17,
    body: '- 허리가 불편한 분 두 명: 앞으로 숙이는 자세는 무릎을 굽혀서\n- 임신 중인 분 한 명: 엎드리는 자세 빼기\n' },
  { id: 'dev-doc-member-tue', folder: '회원 메모', title: '화요일 아침반', summary: '출근 전이라 50분 안에 끝내기.', kind: 'note', status: 'draft', day: 13,
    body: '8시 전에 끝나야 하는 분이 많다. 마무리를 짧게.\n' },
  { id: 'dev-doc-member-new', folder: '회원 메모', title: '이번 달 새로 온 분', summary: '11월에 등록한 수련생 여섯 명의 첫 수업 메모.', kind: 'note', status: 'draft', day: 11,
    body: '처음 수업에서 느낀 점과 다음 시간에 챙길 것을 적어 둔다.\n' },
  { id: 'dev-doc-plan', folder: '', title: '올해 남은 수업 계획', summary: '12월까지 주제와 특강 날짜.', kind: 'note', status: 'draft', day: 9,
    body: '- 12월 첫째 주: 고관절 주간\n- 12월 셋째 주: 연말 특강(명상 90분)\n' },
]

const edits = new Map<string, { title: string; summary: string; body: string; at: string }>()

export function isDevSampleDocument(id: string) {
  return id.startsWith('dev-doc-')
}

export function devSampleEntries(): ZetListEntry[] {
  return SAMPLES.map((item) => {
    const edited = edits.get(item.id)
    return { id: item.id, title: edited?.title ?? item.title, summary: edited?.summary ?? item.summary, kind: item.kind, status: item.status, createdAt: stamp(item.day, 9), updatedAt: edited?.at ?? stamp(item.day), folder: item.folder }
  })
}

export function devSampleDocument(id: string): ZetDocument {
  const item = SAMPLES.find((entry) => entry.id === id)
  if (!item) throw new Error('글을 찾지 못했습니다')
  const edited = edits.get(id)
  return {
    head: { id, title: edited?.title ?? item.title, summary: edited?.summary ?? item.summary, kind: item.kind, status: item.status, created_at: stamp(item.day, 9), updated_at: edited?.at ?? stamp(item.day), assets: [], edges: [] },
    body: edited?.body ?? item.body,
    contentHash: `dev-${id}-${edited?.at ?? 'start'}`,
  }
}

export function saveDevSample(id: string, changes: { title?: string; summary?: string; body?: string }) {
  const current = devSampleDocument(id)
  edits.set(id, { title: changes.title ?? current.head.title, summary: changes.summary ?? current.head.summary, body: changes.body ?? current.body, at: new Date().toISOString() })
}

export const devSampleLinked: ZetLinked = { assets: [], outgoing: [], incoming: [], receipts: [] }
export const devSampleHistory: ZetHistoryEntry[] = []
