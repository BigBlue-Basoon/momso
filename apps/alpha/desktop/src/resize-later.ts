// 크기 관찰을 다음 화면 그림으로 미룬다(2026-11-18).
// ResizeObserver 의 알림 안에서 곧바로 상태나 스타일을 바꾸면, 그 바뀜이 같은 화면 그림 안에서 지켜보던 칸의 크기를 다시 바꿀 수 있다.
// 그러면 브라우저가 "ResizeObserver loop completed with undelivered notifications" 경고를 낸다(설정을 닫고 패널로 돌아올 때 보였음).
// 알림은 모아 두었다가 다음 requestAnimationFrame 에 한 번만 부른다.
export function observeSize(element: Element, onResize: (entry: ResizeObserverEntry) => void) {
  let frame = 0
  let last: ResizeObserverEntry | null = null
  const observer = new ResizeObserver((entries) => {
    last = entries[entries.length - 1] ?? last
    if (frame) return
    frame = window.requestAnimationFrame(() => {
      frame = 0
      if (last) onResize(last)
    })
  })
  observer.observe(element)
  return () => {
    observer.disconnect()
    if (frame) window.cancelAnimationFrame(frame)
    frame = 0
  }
}
