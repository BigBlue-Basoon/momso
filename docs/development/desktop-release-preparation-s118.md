# 데스크톱 설치 파일 릴리스 준비 (S118)

**2026-11-24 결정:** 개발자: “인증서는 돈 안낼래.” Windows 코드 서명 인증서는 구매하지 않고, 설치 파일은 Windows 인증서 서명 없이 낸다. SmartScreen 경고를 받아들인다. 자동 업데이트용 Tauri 서명은 유지하며, 이 서명에 쓰는 개인키는 개발자가 직접 다룬다. Windows 인증서 구매나 유료 서명 서비스 가입은 릴리스 선행 조건이 아니다.

## 현재 확인한 구성

| 항목 | 현재 |
| --- | --- |
| 앱 버전 | `0.1.1` (Tauri와 npm 설정) |
| 설치 파일 | 현재 사용자용 NSIS x64 `momso_0.1.1_x64-setup.exe`. MSI는 현재 빌드 대상이 아니다. |
| 설치 파일 서명 | Windows Authenticode 인증서 없음. 아래 경고를 참고한다. |
| 업데이트 서명 | Tauri 공개키는 `tauri.conf.json`에 있다. 개인키는 개발자가 따로 보관하며 이 작업에서 읽지 않았다. |
| 업데이트 주소 | `https://github.com/BigBlue-Basoon/momso/releases/latest/download/latest.json` |
| 공개 CI | unsigned NSIS, SHA-256, 빌드 출처 JSON을 Actions 산출물로 보관한다. 공개 릴리스가 아니다. |
| 공개 자동화 | `desktop-release.yml.disabled`는 꺼져 있다. 활성화하거나 비밀값을 올리지 않았다. |

2026-11-24에 공개 릴리스 목록을 읽어 확인했을 때 anywhere 개발용 프리릴리스 4개만 있었고 `releases/latest`는 404였다. 데스크톱 정식 릴리스가 만들어져 필요한 파일이 올라가기 전까지 현재 다운로드 버튼과 업데이트 최신 주소는 완성된 경로가 아니다.

같은 날 작업 PC의 첫 제품 릴리스 빌드는 Windows 빌드 잠금의 최대 사용 시간에 도달해 앱 본체 컴파일 중 중단했다. 전체 7-Zip은 사용하지 않기로 했다. 실제 제품 설치 파일 검사와 서명 전 검증이 성공하기 전에는 이 문서를 릴리스 가능 판정으로 삼지 않는다.

이후 제품 NSIS를 만들고 임시 설치해 검사했을 때 앱 실행 파일에서 빌드 PC의 사용자 폴더 경로가 발견돼 서명 준비를 거절했다. Rust와 MSVC의 경로 치환을 적용하고 제품판에 불필요한 개발용 경로를 뺀 뒤 다시 빌드해, 설치된 파일과 묶인 원본 3,763개 항목에서 현재 목록의 사용자 경로·비공개 용어와 알려진 키 무늬를 찾지 못했다. [Rust 경로 치환](https://doc.rust-lang.org/rustc/remap-source-paths.html)은 최선의 노력 방식이므로 결과 파일 검사를 대신하지 않는다. 검사 중에는 실제 제품 앱을 실행하지 않았다. 이 PC에는 기존 `momso` 자료가 있고 같은 Windows 사용자로 제품을 실행하면 실제 보관함과 자료에 닿을 수 있기 때문이다. 별도 사용자 환경에서 제품 실행·종료를 확인해야 한다.

최종 코드로 만든 설치 파일의 SHA-256은 `6d18ee616b9c2dfeea555d7ce486e8316a3899bbec5d8620a80cf8d2b319e04d`다. 본부가 기존 Windows 사용자 사용을 명시적으로 승인한 뒤 실제 설치 파일을 임시 설치하고 별도 화면에서 첫 화면이 그려지는 것을 확인한 뒤 제거했다. 보강된 비공개 목록으로 설치된 파일과 묶인 원본 3,763개를 다시 검사했다. 기존 몸소 보관 폴더는 변경되지 않았고 앱 설정·로컬 자료의 메타데이터 변경은 80건이었다. 변경 전후 값은 사용자 컴퓨터의 별도 기록에 남겼으며 값의 내용은 이 문서에 쓰지 않는다. 앞선 검사 직후 실행 환경이 재시작되어 남은 제거 등록은 시험 위치를 확인한 후 정리했다. 릴리스 구성의 Rust clippy와 공개 위생 검사는 통과했다.

환경 변수만 바꿔서는 실제 제품을 충분히 격리하지 못한다. Tauri 2.11.5의 `app_config_dir`는 `dirs::config_dir()`을 쓰고, Windows 구현은 환경 변수가 아닌 Windows의 `FOLDERID_RoamingAppData`를 조회한다. 제품의 원래 식별자로 실행하면 기존 설정 폴더를 사용할 수 있다. 별도 데스크톱은 화면 초점만 격리하고 Windows 사용자 보관함은 격리하지 않는다. 현재 사용자의 자료를 쓰는 시험은 소유자가 그 사실을 알고 승인했을 때만 수행한다.

2026-11-24 S118b에서 본부가 비공개 이름 목록을 보강한 뒤 같은 설치 파일을 다시 임시 설치해 파일과 묶음 원본 3,763개를 검사했다. 설치 파일 SHA-256은 변하지 않았고 검사 기록의 목록 SHA-256을 갱신했으며 기존 실제 첫 화면 확인 기록을 유지했다. **이 설치 파일은 공개하지 않는다.** 빌드 당시 작업 브랜치의 코드 끝은 `4af3efbb1`이었고, 공개 `main` `3e39afbd5171`의 트리는 스냅샷 `a2ff88c64809`과 일치하지만 그 빌드 트리와 다르다. 데스크톱 화면 소스는 빌드 쪽 21개·공개 쪽 126개로 공통 파일 중 10개도 다르고, Rust의 `provider_install.rs`, `r2_oauth.rs`, 빌드 스크립트와 패키지 잠금 파일도 다르다. 화면 폴더와 생성 런타임처럼 Git에 기록되지 않는 입력의 출처도 따로 검증해야 한다. 줄1의 다음 스냅샷이 공개된 뒤 정확한 공개 소스에서 설치 파일을 새로 빌드하고 검사·실행 확인을 다시 해야 한다. 위 서명 명령이 기술적으로 동작할 수 있어도 이 예전 설치 파일에는 사용하지 않는다.

격리 설치 시험 스크립트 `scripts/test-desktop-release-smoke-s118.py`는 별도 제품 식별자와 시험 자료 폴더, 가짜 자격 증명 저장소를 사용한다. 가짜 저장소를 활성화하려고 release 최적화 빌드의 앱 크레이트에 시험 기능과 debug assertions를 켜므로 완전히 동일한 공개 바이너리의 검증은 아니다. 2026-11-24 그 시험판을 만들고 `scripts/test-desktop-release-uninstall-s118.py`로 설치 → 별도 Windows 데스크톱의 창 응답 → 제거를 한 흐름으로 확인했다. 이 차이는 공개할 실제 설치 파일 검사를 대신하지 않는다.

기존 [서명 결정](windows-distribution-signing-decision.md)은 2026-07-12 당시 기록이며, 업데이터가 비활성이라는 표현은 현재 코드 상태가 아니다. [업데이트 계약](desktop-update-s69.md)에 실제 화면 명령과 진행 중 턴 처리·복구를 적었다. 업데이트용 서명은 Windows의 배포자 인증서가 아니다.

별도로 지정된 `momso-signing-decision` 작업 폴더는 현재 작업트리 내용 없이 이름표만 남아 있어 그 폴더의 파일은 읽을 수 없었다. Git에 남은 서명 결정 문서를 기준으로 대조했다. S54 이후의 현재 빌드 경로는 `build-desktop.mjs`의 Node·CRT 확인과 설치 파일 검사다.

## 본부가 서명 전에 할 일

1. 정확히 공개할 소스 커밋·버전과 라이선스 고지, Windows 앱의 테스트 결과를 확인한다. `npm ci`, `npm run desktop:version:check`, `npm run desktop:build`로 NSIS를 빌드한다. `CARGO_BUILD_JOBS=1`과 작업용 `CARGO_TARGET_DIR`을 지정할 수 있다. `--updater`는 이 단계에 주지 않는다. 일반 빌드는 설치 파일을 열거나 7-Zip을 요구하지 않는다. 설치 내용 검사와 실행 확인은 아래 릴리스 준비에서 한다.
2. `python scripts/check-public-hygiene.py`를 공개할 소스에서 실행한다. 설치 파일의 추가 비공개 용어는 사용자 외부 파일에 한 줄씩 적고 `--private-terms-file`로 지정한다. 용어 자체는 기록에 남기지 않는다. 임시 설치된 제품 파일과 빌드 때 묶은 원본 폴더를 검사한다. 사용자 폴더 경로, 지정 용어, 알려진 키 무늬를 찾고 설치된 앱이 빌드된 앱과 같은지도 확인한다. 릴리스 준비의 실제 실행 확인은 임시 설치된 파일의 Node와 Visual C++ 런타임 DLL 및 의존성도 검사한다. 다른 인코딩이나 알려지지 않은 값까지 없음을 증명하는 검사는 아니므로 최종 공개 검수도 필요하다. 7-Zip 없이 설치 후 파일을 검사하므로 설치되지 않는 NSIS 내부 부분은 확인하지 못한다.

```powershell
python scripts/desktop-release-s118.py prepare --installer "<설치 파일>" --private-terms-file "<비공개 용어 파일>" --installed-dir "<임시 설치 폴더>" --built-exe "<빌드한 앱 실행 파일>" --staged-dir "<묶인 Node 원본 폴더>" --staged-dir "<묶인 고지 원본 폴더>"
```

3. 제품 실행은 임시 Windows 사용자에서 확인하는 것을 기본으로 한다. 본부가 기존 사용자의 시험 자료 사용을 명시적으로 승인한 이번 경우에는 `scripts/verify-product-current-user-s118.py`로 실제 첫 화면·제거를 확인했다. 창 응답뿐 아니라 웹뷰 문서의 준비 상태와 화면 내용도 확인했다. 결과는 운영자의 확인을 전제로 하며 별도 암호화 서명된 증명은 아니다. 다른 설치 파일을 검사할 때는 별도 사용자용 `scripts/verify-product-startup-s118.py`를 사용할 수 있다. 결과 파일은 다음 명령으로 설치 파일의 지문과 대조한다.

```powershell
python scripts/desktop-release-s118.py record-startup --installer "<설치 파일>" --private-terms-file "<비공개 용어 파일>" --startup-evidence "<결과 JSON>"
```

실행 확인 전에는 서명 명령도 중단된다. 시험 환경을 만드는 것은 이 준비 도구가 하지 않는다.

## 개발자가 자기 터미널에서 할 한 줄

서명하지 않은 동일 설치 파일과 위의 동일한 비공개 용어 파일을 지정한다. 아래 명령을 **개발자 본인이** 실행하고 묻는 암호를 입력한다. 키 내용은 명령줄에 넣지 않으며, 서명 도구는 키를 그 위치에서 직접 연다. 암호는 서명 자식 프로세스의 환경에 잠시 전달된다. 준비 영수증의 설치 파일 해시가 달라지면 서명하지 않는다.

```powershell
python scripts/desktop-release-s118.py sign --installer "<설치 파일의 전체 경로>" --private-terms-file "<비공개 용어 파일의 전체 경로>" --key-path "<서명 열쇠 파일 경로>"
```

## 본부가 서명 뒤 할 일

1. 설치 파일 옆의 `.exe.sig`와 `latest.json`이 만들어졌는지 확인한다. 자동 생성된 `latest.json`의 `version`, `platforms.windows-x86_64.signature`, `url`을 확인한다. 서명이 앱의 공개키와 짝인지 반드시 실제로 검증한다. 예: `MOMSO_RELEASE_FILE`을 그 설치 파일로 지정하고 `cargo test --manifest-path apps/alpha/desktop/src-tauri/Cargo.toml verify_release_signature -- --ignored --test-threads=1`. 이 검증은 Rust 빌드 잠금을 지킨다. 서명 스크립트의 성공만으로 공개키 일치를 주장하지 않는다.
2. 준비 영수증의 SHA-256을 다시 비교하고 `node scripts/create-desktop-artifact-metadata.mjs "<설치 파일이 든 폴더>"`의 산출물 및 필요한 제3자 고지를 확인한다. 이 메타데이터의 `signatureStatus: unsigned`는 Windows 코드 서명이 없다는 뜻이며 Tauri 서명의 존재와 별개다. 서명 후 파일이 바뀌면 다시 준비하고 다시 서명한다.
3. 승인된 `v0.1.1` 태그의 **정식 릴리스**에 `momso_0.1.1_x64-setup.exe`, `momso_0.1.1_x64-setup.exe.sig`, `latest.json`, SHA-256과 필요한 고지를 올린다. 초안이나 프리릴리스로만 두면 `releases/latest`에서 보이지 않는다. 공개 전 사용자 승인과 공개 검수는 별도다.
4. 공개 후 실제 `releases/latest`, 설치 파일 URL과 `latest.json`을 확인하고 시험용 이전 설치판에서 업데이트·서명 검증을 확인한다. 태그·릴리스 생성과 공개는 이 준비 작업이 수행하지 않는다.

## 두 제품의 릴리스가 섞일 때

GitHub의 `releases/latest`는 저장소 전체에서 하나다. 현재 anywhere **프리릴리스**는 데스크톱 정식 릴리스를 가리지 않는다. anywhere 작업자는 프리릴리스 여부와 `release.json`·서명을 함께 확인하므로 데스크톱의 `latest.json`만 있는 릴리스를 고르지 않는다. 혼합 목록 회귀 시험은 `test_desktop_stable_release_does_not_hide_anywhere_release`다.

anywhere 릴리스는 `python scripts/momso-release-channels.py publish-anywhere --tag <anywhere-태그> <파일들>`을 사용한다. 이 명령은 정식 릴리스에도 `--latest=false`를 고정한다. 공개 뒤 `python scripts/momso-release-channels.py verify-desktop --tag v0.1.1`로 latest의 태그와 설치 파일·`latest.json`을 확인한다. 사람이 GitHub 화면이나 다른 명령으로 릴리스를 만들면 이 보장이 적용되지 않으므로 같은 확인을 다시 해야 한다. 소개 사이트 코드는 이 작업트리에 위 버튼 구현이 없어 실제 화면 코드는 별도 확인한다.

## Windows의 경고와 선택지

서명되지 않은 EXE는 Windows에서 인식되지 않은 앱으로 경고할 수 있고 조직 정책에서는 실행이 막힐 수도 있다. 업데이트 서명은 이 경고를 없애지 않는다. Microsoft에 따르면 유효한 코드 서명을 해도 새 파일은 평판이 쌓이기 전 경고가 날 수 있으며 EV 인증서도 즉시 통과를 보장하지 않는다. [SmartScreen 공식 설명](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).

현재 정한 길은 Windows 인증서 서명 없는 NSIS다. SmartScreen 경고와 조직 정책에 따른 차단 가능성을 받아들이며, 업데이트의 진위 확인에는 별도의 Tauri 서명을 사용한다. [Microsoft 코드 서명 선택지](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)는 참고 자료다.

근거: [Tauri 업데이터](https://v2.tauri.app/plugin/updater/), [GitHub 최신 릴리스 주소](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases), [GitHub 릴리스 API](https://docs.github.com/en/rest/releases/releases).
