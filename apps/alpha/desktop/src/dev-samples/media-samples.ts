// 개발 앱에서만 보이는 생성물(2026-11-18 김성균 개발자 요청: 사진과 동영상 화면에 피드백을 주려면 무언가 보여야 함).
// MediaView 가 import.meta.env.DEV 일 때만 불러온다. 설치판(vite build)에는 이 파일과 그림·동영상이 들어가지 않는다.
// 그림은 직접 그린 SVG, 동영상은 ffmpeg 의 무늬 생성기로 만든 것이라 저작권 문제가 없다.
import type { MediaItem } from '../media'

const url = (name: string) => new URL(`./${name}`, import.meta.url).href

type Sample = Omit<MediaItem, 'path' | 'posterPath'> & { file: string; poster?: string }

const at = (day: number, hour: number, minute: number) => new Date(2026, 10, day, hour, minute).toISOString()
const codex = { kind: 'ai' as const, provider: 'codex' as const, model: 'gpt-image-1' }
const gemini = { kind: 'ai' as const, provider: 'gemini' as const, model: 'veo-3' }

const SAMPLES: Sample[] = [
  // 아침 스튜디오: 처음 만든 것 → 같은 프롬프트로 다시 → 고친 것
  { id: 'dev-media-studio-1', kind: 'image', title: null, createdAt: at(17, 9, 10), width: 1200, height: 800, durationSec: null, chatId: null, by: codex, how: 'created', at: at(17, 9, 10), file: 'studio-1.svg',
    prompt: '아침 햇살이 드는 요가 스튜디오, 매트 위에서 나무 자세를 하는 사람, 따뜻한 색감, 수업 홍보 포스터용', lineageId: 'studio', version: 1, versionOf: null, versionHow: null },
  { id: 'dev-media-studio-2', kind: 'image', title: null, createdAt: at(17, 9, 14), width: 1200, height: 800, durationSec: null, chatId: null, by: codex, how: 'created', at: at(17, 9, 14), file: 'studio-2.svg',
    prompt: '아침 햇살이 드는 요가 스튜디오, 매트 위에서 나무 자세를 하는 사람, 따뜻한 색감, 수업 홍보 포스터용', lineageId: 'studio', version: 2, versionOf: 'dev-media-studio-1', versionHow: 'regenerated' },
  { id: 'dev-media-studio-3', kind: 'image', title: null, createdAt: at(17, 9, 21), width: 1200, height: 800, durationSec: null, chatId: null, by: codex, how: 'edited', at: at(17, 9, 21), file: 'studio-3.svg',
    prompt: '배경을 더 밝게, 왼쪽에 글자를 넣을 수 있게 여백을 두고 사람은 오른쪽으로', lineageId: 'studio', version: 3, versionOf: 'dev-media-studio-2', versionHow: 'edited' },
  // 바닷가 단체 요가: 두 번 만듦
  { id: 'dev-media-beach-1', kind: 'image', title: null, createdAt: at(16, 20, 2), width: 1080, height: 1080, durationSec: null, chatId: null, by: codex, how: 'created', at: at(16, 20, 2), file: 'beach-1.svg',
    prompt: '해 질 녘 바닷가에서 하는 단체 요가, 넓은 하늘, 인스타그램 정사각형', lineageId: 'beach', version: 1, versionOf: null, versionHow: null },
  { id: 'dev-media-beach-2', kind: 'image', title: null, createdAt: at(16, 20, 6), width: 1080, height: 1080, durationSec: null, chatId: null, by: codex, how: 'created', at: at(16, 20, 6), file: 'beach-2.svg',
    prompt: '해 질 녘 바닷가에서 하는 단체 요가, 넓은 하늘, 인스타그램 정사각형, 사람을 다섯 명으로', lineageId: 'beach', version: 2, versionOf: 'dev-media-beach-1', versionHow: 'edited' },
  // 시간표 배경: 초록 → 파랑으로 고침
  { id: 'dev-media-lotus-1', kind: 'image', title: null, createdAt: at(15, 14, 30), width: 1200, height: 1500, durationSec: null, chatId: null, by: codex, how: 'created', at: at(15, 14, 30), file: 'lotus-1.svg',
    prompt: '연꽃과 잔잔한 물결 무늬, 차분한 초록색, 수업 시간표 배경으로 쓸 세로 그림', lineageId: 'lotus', version: 1, versionOf: null, versionHow: null },
  { id: 'dev-media-lotus-2', kind: 'image', title: null, createdAt: at(15, 14, 35), width: 1200, height: 1500, durationSec: null, chatId: null, by: codex, how: 'edited', at: at(15, 14, 35), file: 'lotus-2.svg',
    prompt: '색을 파란색 계열로 바꿔 줘', lineageId: 'lotus', version: 2, versionOf: 'dev-media-lotus-1', versionHow: 'edited' },
  // 한 번만 만든 것
  { id: 'dev-media-meditation-1', kind: 'image', title: null, createdAt: at(14, 21, 40), width: 1200, height: 800, durationSec: null, chatId: null, by: codex, how: 'created', at: at(14, 21, 40), file: 'meditation-1.svg',
    prompt: '보라색 노을 앞에서 명상 자세로 앉은 사람의 실루엣, 저녁 명상 수업 안내용', lineageId: 'meditation', version: 1, versionOf: null, versionHow: null },
  // 동영상: 아침 하늘 두 번, 물결 세로 한 번
  { id: 'dev-media-sky-1', kind: 'video', title: null, createdAt: at(17, 10, 5), width: 1280, height: 720, durationSec: 4, chatId: null, by: gemini, how: 'created', at: at(17, 10, 5), file: 'sky-1.mp4', poster: 'sky-1-poster.jpg',
    prompt: '천천히 밝아지는 아침 하늘, 4초, 수업 안내 영상의 첫 장면', lineageId: 'sky', version: 1, versionOf: null, versionHow: null },
  { id: 'dev-media-sky-2', kind: 'video', title: null, createdAt: at(17, 10, 12), width: 1280, height: 720, durationSec: 4, chatId: null, by: gemini, how: 'created', at: at(17, 10, 12), file: 'sky-2.mp4', poster: 'sky-2-poster.jpg',
    prompt: '천천히 밝아지는 아침 하늘, 4초, 수업 안내 영상의 첫 장면', lineageId: 'sky', version: 2, versionOf: 'dev-media-sky-1', versionHow: 'regenerated' },
  { id: 'dev-media-waves-1', kind: 'video', title: null, createdAt: at(16, 11, 0), width: 720, height: 1280, durationSec: 4, chatId: null, by: gemini, how: 'created', at: at(16, 11, 0), file: 'waves-1.mp4', poster: 'waves-1-poster.jpg',
    prompt: '물결이 잔잔히 움직이는 배경, 세로 영상(릴스용)', lineageId: 'waves', version: 1, versionOf: null, versionHow: null },
]

export function devSampleMedia(): MediaItem[] {
  return SAMPLES.map(({ file, poster, ...item }) => ({ ...item, path: url(file), posterPath: poster ? url(poster) : null }))
}
