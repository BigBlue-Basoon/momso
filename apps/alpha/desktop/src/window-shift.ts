/**
 * 창 크기가 한 번에 크게 바뀔 때(최대화 · 이전 크기로 · 화면 가장자리에 붙이기) 첫 화면의 내용을 잠깐 숨긴다.
 *
 * Windows는 이런 전환을 "바뀌기 전 창의 그림"과 "새 크기의 창"을 겹쳐 섞으면서 보여 준다. 두 그림은 배율과
 * 위치가 서로 달라서, 가운데에 놓인 CI · 문구 · 단추 · 띠가 그동안 두 겹으로 어긋나 보인다. 웹 화면이 새 크기로
 * 다시 그려지는 것도 몇 장면 늦어서, 그 사이에는 예전 화면이 엉뚱한 자리에 한 번 더 보인다.
 * 전환하는 동안 내용을 숨기면 겹칠 것이 없다. 창이 자리를 잡으면 내용이 새 자리에서 부드럽게 다시 나타난다.
 *
 * - 창 단추로 바꿀 때: 내용을 먼저 부드럽게 숨긴 다음 창 크기를 바꾼다(prepareWindowShift).
 * - 그 밖의 경우(단축키, 제목 표시줄을 두 번 누르기 등): 크기가 바뀐 것을 알아챈 바로 그 장면부터 숨긴다.
 * - 창 가장자리를 끌어 조금씩 크기를 바꿀 때는 숨기지 않는다.
 *
 * 숨기고 보이는 모양은 FirstLaunch.css가 정한다(html의 data-window-shift 값: out = 부드럽게, cut = 바로).
 * 이 기능을 쓰는 화면(watchWindowShift를 부른 화면)이 없으면 아무 일도 하지 않는다.
 */
const FADE_OUT = 80 // 미리 숨길 때 사라지는 시간(ms). FirstLaunch.css의 값과 같아야 한다.
const SETTLE = 220 // 마지막으로 크기가 바뀐 뒤, Windows의 창 전환이 끝나기를 기다리는 시간(ms)
const JUMP = 120 // 한 번에 이만큼(가로 + 세로, px) 넘게 바뀌면 끌어서 바꾼 것이 아니라고 본다
const GIVE_UP = 600 // 미리 숨겼는데 창 크기가 바뀌지 않으면 이 시간(ms) 뒤에 다시 보인다

let watchers = 0
let timer = 0
let width = 0
let height = 0

function setShift(state: 'out' | 'cut' | null) {
  const root = document.documentElement
  if (state) root.dataset.windowShift = state
  else delete root.dataset.windowShift
}

function showAfter(delay: number) {
  window.clearTimeout(timer)
  timer = window.setTimeout(() => setShift(null), delay)
}

function onResize() {
  const jump = Math.abs(window.innerWidth - width) + Math.abs(window.innerHeight - height)
  width = window.innerWidth
  height = window.innerHeight
  const hidden = document.documentElement.dataset.windowShift !== undefined
  if (!hidden && jump < JUMP) return
  if (!hidden) setShift('cut')
  showAfter(SETTLE)
}

/** 첫 화면이 떠 있는 동안 창 크기 변화를 지켜본다. 돌려주는 함수를 부르면 그만둔다. */
export function watchWindowShift() {
  if (watchers++ === 0) {
    width = window.innerWidth
    height = window.innerHeight
    window.addEventListener('resize', onResize)
  }
  return () => {
    if (--watchers > 0) return
    window.removeEventListener('resize', onResize)
    window.clearTimeout(timer)
    setShift(null)
  }
}

/** 창 크기를 바꾸기 직전에 부른다. 내용이 다 숨겨지면 끝난다(숨길 화면이 없으면 바로 끝난다). */
export function prepareWindowShift() {
  if (watchers === 0) return Promise.resolve()
  setShift('out')
  showAfter(GIVE_UP)
  // 다 숨겨진 장면이 실제로 화면에 올라간 뒤에 창을 바꾸도록 조금 더 기다린다.
  // 너무 일찍 바꾸면 덜 사라진 내용이 "바뀌기 전 창의 그림"에 옅게 남는다.
  return new Promise<void>((resolve) => window.setTimeout(resolve, FADE_OUT + 50))
}
