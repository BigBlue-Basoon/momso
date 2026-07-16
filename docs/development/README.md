# 개발 문서

이 폴더는 `momso` 저장소의 개발, 인프라, 실행 방법, 구현 로그를 정리하는 공간입니다.

## 문서

- **`20260711_desktop_alpha_transition.md`: 알파를 Windows 데스크톱 앱(Tauri + SQLite)으로 전환한 결정과 정황 — 현재 개발의 기준.** 설명 웹(`apps/alpha/site`)과 실제 알파(`apps/alpha/desktop`)를 분리하고, 사용자 컴퓨터를 1차 기록 보관소로 삼는다.
- `20260602_momso_developer_surface_baseline.md`: 개발자 표면(API·CLI·MCP) 기준선
- `20260325_Infrastructure.md`: Supabase, Vercel, 환경 변수, 보안 규칙 같은 인프라 기준 *(웹 프로토타입 시기)*
- `20260325_WebAppGuide.md`: 프로토타입 앱 실행/확인 가이드 *(웹 프로토타입 시기)*
- `20260325_ImplementationLog.md`: 구현 이력과 검증 기록 *(웹 프로토타입 시기)*

> ⚠️ `20260325_*` 세 문서는 **웹 프로토타입 시기의 기록**입니다. 2026-07-11 데스크톱 전환 이후의 현재 기준은 위 전환 문서를 봅니다.

## 사용 원칙

- 로컬 실행, 배포, 환경 변수, 데이터베이스 작업은 이 폴더에서 먼저 확인합니다.
