# momso Windows desktop alpha

실제 momso 알파가 실행될 Windows 데스크톱 앱이다. Tauri 2의 Rust 셸, React/Vite UI,
로컬 SQLite로 구성한다.

## 실행

루트에서 다음 명령을 사용한다.

```powershell
npm install
npm run desktop:ui    # UI만 브라우저에서 확인
npm run desktop:dev   # 실제 Tauri 창과 로컬 저장소 확인
npm run desktop:build # NSIS 설치 파일 생성
```

Windows 개발 환경에는 Rust stable MSVC, Microsoft C++ Build Tools, WebView2가 필요하다.
루트에서 `npm test --workspace @momso/alpha-desktop` 및
`cargo test --manifest-path apps/alpha/desktop/src-tauri/Cargo.toml`로 테스트한다.

앱은 AI 응답 평가를 수집하거나 전송하지 않는다.

## 현재 범위

- 앱 시작 시 AppConfig의 DB·원본·내보내기·백업 폴더와 별도 AppLogDir의 로그 폴더 생성
- Rust 전용 SQLite 연결, 원자적 마이그레이션, 건강 검사
- 마이그레이션 전 온라인 백업과 수동 백업 명령
- 본문과 요약을 분리한 momso 자체 기록 저장 구조
- `초안 → 검토 필요 → 검토 완료` 상태와 사람 확인 뒤 확정하는 변경 계획
- 지우지 않는 버전 이력, 과거 버전을 새 버전으로 되살리는 기록 복원
- 요약 목록에서는 본문을 읽지 않고, 요약 점검 때 필요한 본문만 다시 읽는 검사
- 녹음·전사·PDF 원본의 내용 주소형 보존, 중복 제거, 정확한 기록 버전 연결
- 원문과 기록 본문을 전체 해시에 묶어 UTF-8 페이지 단위로 읽는 백엔드 계약
- 중단·재시작을 기록하는 장기 작업 원장과 취소 가능한 원본 무결성 검사
- 과거 백업 성공 기록과 현재 원격 존재 여부를 구분하는 로컬 백업 증거
- 중복 앱 실행 방지와 회전 로그
- 실제 경로와 준비 상태를 보여주는 기반 화면
- CI 설치본의 SHA-256과 빌드 출처 manifest 생성

- Windows 파일 선택 대화상자·끌어 놓기로 받은 경로를 Rust 보존 계획(해시·크기·중복 여부)으로 미리 보여주고, 확정하면 원본을 해시 이름으로 보존한 뒤 새 기록 버전에 연결

WOM 런타임·파일 형식·내부 용어는 채택하지 않는다. 원본 파일 보존 백엔드와 사용자 소유 원격 백업은
구현했지만, 보존된 원본을 기록 화면에서 다시 읽는 검수 화면, 실제 대용량 자료와 실제 사용자 계정 통합 검증은 아직 남아 있다.
