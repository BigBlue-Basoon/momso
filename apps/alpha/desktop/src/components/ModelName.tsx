import { modelNameView } from '../words'

/**
 * 모델 이름을 그리는 한 가지 모양(2026-11-16 김성균 개발자가 정함, 사진 495 의 "나"):
 * 이름이 주인공이고 판 번호는 바로 뒤에 작고 흐리게. 모델 이름이 보이는 자리는 모두 이것을 쓴다(단추만 이름만).
 */
export function ModelName({ label, className }: { label: string; className?: string }) {
  const { name, version } = modelNameView(label)
  return (
    <span className={className ? `model-name ${className}` : 'model-name'}>
      {name}
      {version && <span className="model-name-version">{version}</span>}
    </span>
  )
}
