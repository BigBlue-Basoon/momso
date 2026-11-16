# Windows 릴리스 자동화 준비

이 작업 흐름은 `desktop-release.yml.disabled`로 저장되어 있어 실행되지 않습니다. 실제 공개와 활성화는 따로 결정합니다. 업데이트 서명과 Windows 설치 파일의 코드 서명은 서로 다릅니다. 이 방법은 업데이트만 서명하며 Windows 코드 서명은 하지 않습니다.

## 공개하기로 정한 뒤

1. GitHub의 `BigBlue-Basoon/momso`에서 **Settings → Secrets and variables → Actions → New repository secret**을 엽니다. 저장소 관리자 권한이 필요합니다.
2. `TAURI_SIGNING_PRIVATE_KEY`에 직접 보관하는 업데이트 개인 키 파일의 **내용**을 넣습니다. `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`에는 그 키의 암호를 넣습니다. 값은 다른 사람에게 보내거나 채팅에 붙이지 않습니다. 빈 암호는 이 작업 흐름에서 허용하지 않습니다.
3. 검토된 작업 흐름 파일의 이름 끝 `.disabled`를 제거해 `.github/workflows/desktop-release.yml`로 만든 뒤 main에 반영합니다. 그 전에는 자동 실행되지 않습니다.
4. 새 버전 숫자를 `apps/alpha/desktop/package.json`, `apps/alpha/desktop/src-tauri/Cargo.toml`, `apps/alpha/desktop/src-tauri/tauri.conf.json`의 `version`에 똑같이 적습니다. `npm install --package-lock-only --ignore-scripts`로 잠금 파일을 맞추고 `npm run desktop:version:check`로 세 숫자를 확인합니다. 이 명령은 숫자를 바꾸지는 않고 같은지만 확인합니다. 변경을 검토하고 기존 검사 작업이 통과한 커밋을 main에 반영합니다.
5. 그 커밋에 같은 버전의 태그(예: `0.1.1`이면 `v0.1.1`)를 만들고 GitHub에 올리면 자동으로 시작합니다. 이미 태그를 올렸다면 **Actions → Desktop release → Run workflow**에서 태그를 입력하고 **Run workflow**를 눌러 수동으로 시작할 수도 있습니다. 태그가 가리키는 커밋은 main에 들어 있어야 합니다. 풀 리퀘스트에서는 실행되지 않습니다.
6. Actions의 결과와 Releases의 설치 파일·서명·`latest.json` 세 파일을 확인합니다. 기존 시험용 앱에서 업데이트 확인을 누르고 설치 여부를 직접 확인합니다. 새 버전이 이전 버전보다 높아야 합니다. 비밀값 누락, 빌드 또는 서명 실패 시 공개하지 않습니다. 업로드 중 실패하면 비공개 초안이 남을 수 있으므로 확인합니다.

공개 전 실제 설치 파일을 로컬 시험 서버에서 내려받아 앱에 설정된 공개 키로 서명을 확인합니다. 키가 일치하지 않으면 공개하지 않습니다. `desktop-alpha.yml`의 기존 검사와는 별개로 릴리스에 필요한 빌드와 서명만 수행합니다.

개발 중 로컬 시험은 가짜 파일의 다운로드와 서명 확인까지이며 실제 설치나 앱 재시작은 하지 않습니다. 실제 설치는 공개 후 시험용 앱에서 따로 확인합니다.

## Windows 경고

설치 파일에 Windows 코드 서명이 없으면 Microsoft Defender SmartScreen이 “Windows의 PC 보호” 경고를 표시할 수 있습니다. 일부 PC에서는 **추가 정보**를 눌러 **실행**할 수 있으나, 조직 정책에 따라 실행이 막힐 수 있습니다. 파일을 받아도 안전하다고 자동 보증되는 것은 아닙니다.

Microsoft에 따르면 서명 없는 파일은 다운로드 평판이 쌓이면서 경고가 줄 수 있지만, 새 파일마다 다시 쌓여야 하고 경고가 사라진다는 보장은 없습니다. Microsoft Store 배포는 Microsoft가 패키지에 서명하는 별도 방식이며, 스토어 등록과 심사 등의 조건이 있습니다. 이 경로의 무료 이용 가능 여부는 확인하지 못했습니다. 서명만으로 SmartScreen 평판이 즉시 확보되는 것은 아닙니다. 셀프 서명은 일반 사용자 PC가 신뢰하는 코드 서명을 대신하지 않습니다. Windows 11의 Smart App Control이 서명 없는 파일을 차단할 수도 있습니다.

근거: [Tauri updater](https://v2.tauri.app/plugin/updater/), [Tauri GitHub 배포](https://v2.tauri.app/distribute/pipelines/github/), [GitHub Actions 비밀값](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets), [Microsoft SmartScreen 평판](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation), [Microsoft 코드 서명 선택지](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options).
