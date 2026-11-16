export type WorkspaceView = 'inbox' | 'records' | 'review' | 'archive'
export type QueueFilter = 'all' | 'review' | 'done'
export type RecordTab = 'summary' | 'transcript' | 'details'
export type AssistantMode = 'ask' | 'draft' | 'review'
export type RecordStatus = 'needs-review' | 'draft' | 'reviewed'

export type SummarySection = {
  title: string
  body: string
  source: string
}

export type TranscriptLine = {
  time: string
  speaker: string
  text: string
  important?: boolean
}

export type WorkspaceRecord = {
  id: string
  title: string
  preview: string
  dateLabel: string
  time: string
  duration: string
  sourceType: 'audio' | 'note' | 'ai-conversation'
  sourceLabel: string
  participantLabel: string
  status: RecordStatus
  summarySections: SummarySection[]
  transcript: TranscriptLine[]
  createdAt: string
  archiveId: string
  checksum: string
  aiNote: string
  originalFileName?: string
  originalFileSize?: string
  retentionLabel?: string
  externalProvider?: string
  externalConversationId?: string
  projectLabel?: string
  runtimeRecordKind?: string
  runtimeVersion?: number
  runtimeBodySha256?: string
}

export type AssistantMessage = {
  id: string
  role: 'assistant' | 'user'
  text: string
}

export const VIEW_LABELS: Record<WorkspaceView, { title: string }> = {
  inbox: { title: '오늘의 작업함' },
  records: { title: '모든 기록' },
  review: { title: '검수 대기' },
  archive: { title: '확정 기록' },
}

export const WORKSPACE_RECORDS: WorkspaceRecord[] = [
  {
    id: 'evening-hatha-0713',
    title: '저녁 하타 수업 기록',
    preview: '호흡이 안정된 뒤 어깨 가동 범위가 넓어짐',
    dateLabel: '오늘',
    time: '19:32',
    duration: '52분 18초',
    sourceType: 'audio',
    sourceLabel: '수업 녹음',
    participantLabel: '수련생 6명',
    status: 'needs-review',
    summarySections: [
      {
        title: '수업의 흐름',
        body: '초반에는 앉은 호흡으로 속도를 낮춘 뒤, 고관절과 흉곽을 차례로 열었다. 호흡이 길어진 뒤 서 있는 자세로 이동하면서 전체 흐름이 안정됐다.',
        source: '00:08:14–00:14:02',
      },
      {
        title: '주의할 맥락',
        body: '한 수련생이 왼쪽 어깨가 “조금 걸리는 느낌”이라고 표현했다. 통증으로 단정하지 않고 팔을 올리는 범위를 줄여 진행했다.',
        source: '00:31:42–00:34:08',
      },
      {
        title: '다음 수업에서 확인',
        body: '왼쪽 어깨의 느낌이 계속되는지 먼저 묻고, 무리 없이 가능한 팔의 높이를 다시 확인한다. 오늘 사용한 호흡 속도는 유지한다.',
        source: '00:48:20–00:50:11',
      },
    ],
    transcript: [
      {
        time: '08:14',
        speaker: '강사',
        text: '들이쉬는 호흡보다 내쉬는 호흡을 조금 더 길게 가져가 볼게요.',
      },
      {
        time: '12:37',
        speaker: '강사',
        text: '지금부터는 호흡의 속도를 그대로 두고 움직임만 조금씩 넓혀봅니다.',
      },
      {
        time: '31:42',
        speaker: '수련생',
        text: '왼쪽 어깨가 아픈 건 아닌데 위로 갈 때 조금 걸리는 느낌이 있어요.',
        important: true,
      },
      {
        time: '32:03',
        speaker: '강사',
        text: '그러면 오늘은 걸리기 전까지만 올리고, 그 범위를 기억해 둘게요.',
        important: true,
      },
      {
        time: '48:20',
        speaker: '강사',
        text: '다음 시간 시작 전에 어깨 느낌이 어땠는지 한 번 더 확인하겠습니다.',
      },
    ],
    createdAt: '2026년 7월 13일 오후 7:32',
    archiveId: 'momso-local / class-20260713-1932',
    checksum: 'SHA-256 · 6fd0…a12c',
    aiNote: '원문에서 “걸리는 느낌”이라고 한 표현을 그대로 보존했어요. 아직 “통증”이나 “부상”으로 단정하지 않았고, 강사의 대응과 다음 확인 항목을 분리했습니다.',
  },
  {
    id: 'new-student-consult-0713',
    title: '신규 수련생 상담',
    preview: '오래 앉아 있을 때 허리 주변이 굳는 느낌을 이야기함',
    dateLabel: '오늘',
    time: '16:10',
    duration: '18분 06초',
    sourceType: 'audio',
    sourceLabel: '상담 녹음',
    participantLabel: '수련생 1명',
    status: 'draft',
    summarySections: [
      {
        title: '상담에서 말한 목표',
        body: '몸을 유연하게 만드는 것보다 퇴근 뒤 긴장을 낮추고 잠들기 전 몸이 가벼워지는 경험을 우선하고 싶다고 말했다.',
        source: '00:03:11–00:05:24',
      },
      {
        title: '현재 느끼는 불편',
        body: '오래 앉아 있는 날에는 허리 주변이 굳는 느낌이 있다고 말했다. 진단명이나 치료 이력은 이 상담에서 언급되지 않았다.',
        source: '00:07:52–00:09:40',
      },
      {
        title: '첫 수업에서 확인',
        body: '바닥에 앉는 자세와 누운 자세에서 편안함의 차이를 확인하고, 호흡을 길게 유지할 수 있는 강도를 함께 찾는다.',
        source: '00:14:18–00:16:02',
      },
    ],
    transcript: [
      {
        time: '03:11',
        speaker: '수련생',
        text: '유연해지는 것도 좋지만 퇴근하고 긴장이 좀 내려갔으면 좋겠어요.',
      },
      {
        time: '07:52',
        speaker: '수련생',
        text: '오래 앉아 있으면 허리 주변이 굳어 있는 느낌이 들어요.',
        important: true,
      },
      {
        time: '09:40',
        speaker: '강사',
        text: '그 느낌을 통증이라고 표현할 정도인지, 움직일 때 달라지는지도 다음에 천천히 확인할게요.',
      },
      {
        time: '14:18',
        speaker: '강사',
        text: '첫 시간에는 편안한 자세를 찾는 것부터 시작하겠습니다.',
      },
    ],
    createdAt: '2026년 7월 13일 오후 4:10',
    archiveId: 'momso-local / consult-20260713-1610',
    checksum: 'SHA-256 · a14e…03bd',
    aiNote: '상담에서 직접 말하지 않은 진단이나 원인을 추가하지 않았어요. 수련생의 목표, 현재 표현, 첫 수업에서 확인할 일을 각각 나눠 두었습니다.',
  },
  {
    id: 'morning-flow-0713',
    title: '아침 빈야사 수업',
    preview: '균형 자세에서 시선과 발바닥 감각을 반복해서 확인함',
    dateLabel: '오늘',
    time: '09:04',
    duration: '46분 31초',
    sourceType: 'audio',
    sourceLabel: '수업 녹음',
    participantLabel: '수련생 4명',
    status: 'reviewed',
    summarySections: [
      {
        title: '수업의 흐름',
        body: '짧은 태양경배 뒤 균형 자세로 이어졌다. 속도를 높이기보다 발바닥의 세 지점과 시선을 반복해서 확인했다.',
        source: '00:11:02–00:25:18',
      },
      {
        title: '관찰',
        body: '균형이 흔들릴 때 시선을 먼저 고정하면 호흡이 다시 이어지는 경향이 있었다. 개인별 평가로 확정하지 않고 수업 전체 관찰로 남겼다.',
        source: '00:26:10–00:31:44',
      },
      {
        title: '다음 수업에서 이어갈 점',
        body: '같은 균형 자세를 짧게 반복하되, 시선과 발바닥 감각 중 무엇이 더 도움이 되는지 각자 확인하도록 안내한다.',
        source: '00:42:12–00:44:03',
      },
    ],
    transcript: [
      {
        time: '11:02',
        speaker: '강사',
        text: '발바닥 앞쪽과 뒤꿈치가 모두 바닥을 누르는지 확인합니다.',
      },
      {
        time: '26:10',
        speaker: '강사',
        text: '흔들리면 멈추지 말고 시선을 한 곳에 두고 호흡부터 다시 찾아볼게요.',
        important: true,
      },
      {
        time: '42:12',
        speaker: '강사',
        text: '다음 시간에는 어떤 감각이 균형에 더 도움이 되는지 비교해 보겠습니다.',
      },
    ],
    createdAt: '2026년 7월 13일 오전 9:04',
    archiveId: 'momso-local / class-20260713-0904',
    checksum: 'SHA-256 · 144a…fed8',
    aiNote: '이 기록은 강사 검수를 마쳤어요. 수업 전체의 관찰과 개인에 대한 판단을 섞지 않은 상태로 확정돼 있습니다.',
  },
  {
    id: 'weekend-workshop-0712',
    title: '주말 워크숍 메모',
    preview: '보조 도구를 먼저 고르게 했을 때 준비 시간이 짧아짐',
    dateLabel: '어제',
    time: '14:20',
    duration: '메모 9개',
    sourceType: 'note',
    sourceLabel: '현장 메모',
    participantLabel: '참여자 11명',
    status: 'reviewed',
    summarySections: [
      {
        title: '운영 관찰',
        body: '시작 전에 각자 필요한 보조 도구를 고르게 하자 수업 중간의 이동과 대기 시간이 줄었다.',
        source: '메모 2–4',
      },
      {
        title: '참여자 반응',
        body: '자세의 완성보다 선택 가능한 범위를 먼저 안내한 점이 편안했다는 반응이 있었다.',
        source: '메모 6–7',
      },
      {
        title: '다음 운영에 반영',
        body: '워크숍 안내문에 보조 도구 선택 시간을 포함하고, 시작 10분 전 준비 상태를 확인한다.',
        source: '메모 8–9',
      },
    ],
    transcript: [
      { time: '메모 2', speaker: '강사', text: '입장하면서 블록과 스트랩을 직접 고르도록 안내함.' },
      { time: '메모 6', speaker: '참여자', text: '할 수 있는 범위를 먼저 골라도 된다는 설명이 편했다.' },
      { time: '메모 9', speaker: '운영', text: '다음 안내문에는 준비 시간을 별도로 적기.' },
    ],
    createdAt: '2026년 7월 12일 오후 2:20',
    archiveId: 'momso-local / workshop-20260712-1420',
    checksum: 'SHA-256 · 98c2…11f0',
    aiNote: '현장 메모의 관찰, 참여자 발화, 다음 운영 결정을 서로 다른 항목으로 유지했습니다.',
  },
]

export function getStatusLabel(status: RecordStatus) {
  if (status === 'needs-review') return '검수 필요'
  if (status === 'draft') return 'AI 초안'
  return '검수 완료'
}

export function initialMessage(record: WorkspaceRecord): AssistantMessage {
  return {
    id: `${record.id}-intro`,
    role: 'assistant',
    text: record.aiNote,
  }
}
