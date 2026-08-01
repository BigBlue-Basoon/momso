# momso Windows 데스크톱 알파 전환

> 결정일: 2026-07-11 · 앱 경로 개편: 2026-07-15
> 대체하는 설계: [AI Wiki 기술 설계](../planner/briefs/20260621_momso_ai_wiki_design.md)의 클라우드 웹 알파 런타임
> 상태: 채택. 로컬 저장 기반(단계 A) 완료, 기록 수명주기 백엔드(단계 B) 진행 중.

## 1. 한 문장 결정

`momso-alpha.vercel.app`은 제품을 설명하는 공개 웹사이트로 남기고, 실제 momso 알파는 **사용자 컴퓨터를 1차 기록 보관소로 삼는 Windows 데스크톱 앱**으로 만든다.

## 2. 왜 웹 런타임에서 전환하는가

브라우저 웹앱도 로컬 저장소를 쓸 수 있지만, 사용자가 파일 위치와 백업 방식을 이해하고 통제하기 어렵다. 브라우저 저장소는 용량·삭제 정책과 브라우저 구현의 영향을 받고, 운영체제 파일과의 통합도 제한된다.

momso가 지키려는 핵심은 다음 세 가지다.

1. 원본 기록은 서비스 계정이 아니라 사용자가 소유한다.
2. 인터넷이나 중앙 서버가 없어도 기본 기록을 열 수 있어야 한다.
3. 원격 동기화와 AI 전송은 기본값이 아니라 사용자가 선택하는 2차 기능이다.

이 방향은 Ink & Switch의 local-first 원칙 중 로컬 복사본, 오프라인 동작, 장기적 데이터 소유권과 맞닿아 있다. 다만 momso는 그 문구를 선언하는 데서 끝내지 않고, **실제 파일 경로와 내보내기 가능성으로 검증한다.**

## 3. 앱별 역할

| 경로 | 역할 | 데이터를 받는가 |
|---|---|---|
| `apps/alpha/site` | 공개 설명 웹, 제품 방향과 개발 상태 | 로그인·수업 업로드·회원 DB 없음 |
| `apps/alpha/desktop` | 실제 Windows 알파, 기록 생성·검수·보존 | 사용자 PC의 로컬 DB와 파일 폴더 |
| `apps/inbodylike-demo` | 2026-06 지원사업 데모 스냅샷 | 동결된 데모 범위 |

기존 웹 프로토타입 전체는 git 이력에 보존되어 있다. 필요한 코드는 이력에서 참고할 수 있지만, 클라우드 우선 구조를 그대로 복원하지 않는다.

## 4. 채택한 개발 스택

| 층 | 선택 | 이유 |
|---|---|---|
| 데스크톱 셸 | Tauri 2 | Windows 네이티브 설치 파일, Rust 명령, 운영체제 경로 접근 |
| UI | React 19 + TypeScript + Vite | 기존 React 경험을 재사용하고 Tauri가 권장하는 SPA 개발 흐름 사용 |
| 로컬 구조 DB | SQLite (`rusqlite`, Rust 전용 경계) | 단일 파일, 트랜잭션, 마이그레이션, 웹뷰에 SQL 권한을 주지 않는 좁은 경계 |
| 원본 파일 | 앱 로컬 데이터 폴더 | 녹음·전사·PDF 등 큰 객체를 DB와 분리 |
| Windows 배포 | NSIS current-user installer | 관리자 권한 없이 사용자 범위 설치를 기본으로 준비 |

공식 설치 전제와 사용법:

- Tauri Windows prerequisites: https://v2.tauri.app/start/prerequisites/
- Tauri + Vite frontend: https://v2.tauri.app/start/frontend/
- rusqlite: https://docs.rs/rusqlite/latest/rusqlite/
- SQLite Online Backup API: https://www.sqlite.org/backup.html
- Tauri Windows installer: https://v2.tauri.app/distribute/windows-installer/
- local-first 원칙 참고: https://www.inkandswitch.com/essay/local-first/

## 5. 현재 로컬 저장소

앱은 운영체제가 정한 사용자별 경로를 Tauri로 해석하며, **경로를 코드에 하드코딩하지 않는다.** 실제 경로는 앱의 개요 화면에 표시한다.

| 저장물 | 위치 기준 | 현재 용도 |
|---|---|---|
| `momso.db` | Tauri AppConfig | 런타임 메타데이터, momso 기록 현재본, 요약, 버전 이력 |
| `objects/` | Tauri AppConfig 아래 | 향후 원본 객체 보관 |
| `exports/` | Tauri AppConfig 아래 | 사용자가 내보낼 결과물 |
| `backups/` | Tauri AppConfig 아래 | 마이그레이션 전·수동 SQLite 스냅샷 |
| `logs/` | Tauri AppLogDir | 사용자 기록과 분리된 회전 진단 로그 |

DB와 `objects/`, `exports/`, `backups/`는 같은 사용자별 momso AppConfig 루트 아래에 둔다. 앱의 `데이터 폴더` 버튼은 이 루트를 열며, 사용자가 DB와 원본·내보내기 파일을 한 위치에서 확인할 수 있게 한다. 로그는 사용자 기록 백업에 섞이지 않도록 운영체제의 별도 로그 디렉터리에 둔다.

현재 DB에는 요가 수업·회원·관계처럼 아직 합의되지 않은 도메인 테이블을 만들지 않는다. 대신 여러 기록 유형이 공통으로 쓸 수 있는 **최소 수명주기만** 둔다.

## 6. 외부 참고 구현에 대한 경계

로컬 기록 설계를 비교·학습하기 위해 외부 참고 구현을 벤치마크한다. 다만 그것은 아이디어를 점검하는 기준일 뿐, **momso의 런타임 의존성이나 데이터 계약이 아니다.** 참고 구현에서 어떤 아이디어를 가져올 때는 아래를 지킨다.

1. 특정 릴리스의 기능을 그대로 복사하지 않고 해결하려는 문제와 안전 조건을 먼저 적는다.
2. momso 자체 스키마·Rust 명령·테스트로 다시 설계한다.
3. 참고 구현의 내부 용어를 사용자 화면이나 momso 공개 계약에 노출하지 않는다.
4. 로컬 우선, 원본 보존, 사람 검수라는 momso 원칙과 맞는지 테스트한다.
5. 참고 구현의 업데이트는 자동 적용하지 않고 매번 독립적으로 검토한다.

## 7. 단계별 전환 계획

### 단계 A — 기반 준비 (완료)

- 설명 웹에서 로그인·백엔드·업로드·API 제거
- Tauri Windows 셸과 NSIS 빌드 설정
- 로컬 폴더 및 SQLite 초기화
- Rust 전용 DB 권한, 마이그레이션 전 백업, 건강 검사, 중복 실행 방지, 회전 로그
- 외부 기록 엔진 없이 독립 실행되는 로컬 저장 기반
- Windows CI 설치본, 체크섬, 빌드 출처 manifest 보관

### 단계 B — 실제 기록 흐름 (진행 중)

- 본문과 첫 읽기용 요약 분리
- AI 초안과 사람 검수 상태 분리
- 변경 미리보기와 명시적 사람 확인 뒤 원자적 확정
- 지우지 않는 버전 이력과 과거 기록 복원
- 원본 파일 가져오기와 체크섬, 로컬 내보내기는 다음 배치

### 단계 C — 요가 도메인 계층

- 수업, 강사, 수련생 관계의 최소 도메인 모델
- 화자·용어·금기·부상 맥락의 검수 UI
- 기록 간 연결과 종단 조회

### 단계 D — 선택적 외부 연결

- 사용자가 켜는 암호화 백업
- 외부 AI로 전송할 범위의 사전 표시와 동의
- 여러 장치 동기화는 충돌·복구 계약이 생긴 뒤 검토

## 8. 개발과 검증

```powershell
npm install
npm run alpha:build
npm run desktop:ui:build
npm run desktop:version:check
npm run desktop:dev
npm run desktop:build
```

Windows에서 실제 데스크톱 빌드를 완료하려면 다음이 필요하다.

- Rust stable MSVC (`rustup`, `rustc`, `cargo`)
- Visual Studio 2022 C++ Build Tools와 Windows SDK
- Microsoft Edge WebView2 Runtime

**완료 판정은 React 화면이 보이는 것만으로 하지 않는다.** 실제 Tauri 창에서 AppConfig의 `momso.db`, `objects/`, `exports/`, `backups/`와 별도 AppLogDir의 로그가 생성되고, 앱 화면의 경로가 디스크의 경로와 일치하며, NSIS 설치 파일이 만들어져야 한다.

로컬 PC에 C++ Build Tools가 없는 경우에도 PR의 Windows 네이티브 빌드 검사가 같은 명령으로 NSIS 설치 파일까지 빌드한다. 로컬 실사용 검증과 CI 네이티브 빌드는 서로 대체하지 않고 둘 다 최종 배포 전 통과해야 한다.

성공한 PR과 `main` 실행은 서명 전 테스트 설치 파일, SHA-256, 빌드 출처 manifest를 14일간 보관한다. manifest에는 버전·원본 커밋·CI 실행·서명 상태가 들어간다. 이는 내부 실행 검증용이며 공개 배포본이나 서명된 릴리스가 아니다.

## 9. 이번 결정의 비범위

- 외부 참고 구현의 런타임·내부 구현·파일 형식을 제품 의존성으로 넣지 않는다.
- 클라우드 동기화 서버를 만들지 않는다.
- 기존 웹 프로토타입의 클라우드 백엔드/BYOK 구조를 데스크톱 앱에 옮기지 않는다.
- 요가 도메인 UI/UX를 이번 기반 작업에서 확정하지 않는다.
- 코드 서명 인증서와 공개 다운로드 채널은 외부 테스트 범위가 정해진 뒤 결정한다.
