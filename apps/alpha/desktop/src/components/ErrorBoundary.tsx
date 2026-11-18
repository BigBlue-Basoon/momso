import { Component, type ErrorInfo, type ReactNode } from 'react'
import { W } from '../words'

/**
 * 화면 한 부분이 그리다 죽어도 앱 전체가 비지 않게(2026-11-18: 문서 화면이 통째로 비었던 일 뒤). 그 부분 자리에
 * "이 부분을 표시하지 못했습니다 [다시 시도]"를 보이고, 다시 시도를 누르면 그 부분만 다시 그린다.
 * className 으로 원래 그 부분이 놓이던 자리(격자 칸 등)를 그대로 쓴다. 오류는 개발 기록(console)에 남긴다.
 */
export class ErrorBoundary extends Component<{ children: ReactNode; className?: string; name: string }, { failed: boolean }> {
  state = { failed: false }

  static getDerivedStateFromError() {
    return { failed: true }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(`[momso] ${this.props.name} 를 그리지 못함`, error, info.componentStack)
  }

  render() {
    if (!this.state.failed) return this.props.children
    return (
      <div className={['screen-error', this.props.className].filter(Boolean).join(' ')} role="alert">
        <span>{W.app.partFailed}</span>
        <button className="secondary-button" type="button" onClick={() => this.setState({ failed: false })}>{W.firstRun.retry}</button>
      </div>
    )
  }
}
