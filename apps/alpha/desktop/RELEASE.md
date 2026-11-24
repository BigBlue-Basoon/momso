# Windows 릴리스와 업데이트

이 문서는 Windows PowerShell 5.1 기준이다. 첫째 줄의 공개 키와 `src-tauri/tauri.updater-build.conf.json`이 통합된 코드에서만 실행한다. 실제 공개 전에는 별도 승인과 MinGit 직접 다운로드 방식의 설치·라이선스 검토가 필요하다. 설치 파일의 Windows 코드 서명은 없으며 Windows에서 경고가 표시될 수 있다. 업데이트용 서명은 별개이며 반드시 필요하다.

## 빌드와 서명

1. 데스크톱 앱의 버전을 올리고 `npm run desktop:version:check`와 필요한 전체 검사를 통과한다. 사용 중인 앱보다 높은 버전이어야 업데이트로 보인다.
2. 공개 저장소의 루트 폴더에서 PowerShell 5.1을 열고 다음 블록을 실행한다. 개인 키 파일을 안전한 폴더에 보관하고 아래에서 그 파일 경로를 입력한다. 암호는 질문이 나오면 입력하므로 명령 기록에 남지 않는다. 키 파일 내용은 누구에게도 보내지 않는다.

```powershell
$keyPath = Read-Host '서명 열쇠 파일의 전체 경로'
$securePassword = Read-Host '개인 키 암호 (없으면 빈 채로 Enter)' -AsSecureString
$bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
try {
    $env:TAURI_SIGNING_PRIVATE_KEY = $keyPath
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
    npm run desktop:build -- --updater
    if ($LASTEXITCODE -ne 0) { throw '빌드가 실패했습니다.' }
} finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    Remove-Variable securePassword,keyPath -ErrorAction SilentlyContinue
}
```

키 경로는 파일 내용이 아니라 경로다. 암호는 빌드 중 프로세스 환경에 잠시 존재한다. 성공하면 `apps/alpha/desktop/src-tauri/target/release/bundle/nsis/`에 설치 파일 `momso_<버전>_x64-setup.exe`와 같은 이름의 `.exe.sig`가 생긴다. 두 파일은 같은 빌드에서 나온 짝이어야 한다.

## 업데이트 정보와 공개

3. 같은 PowerShell 창에서 아래를 실행한다. 버전과 태그는 빌드한 실제 버전과 같게 입력한다. `.sig`의 내용은 공개용 서명이지 개인 키가 아니다.

```powershell
& {
$ErrorActionPreference = 'Stop'
$version = Read-Host '앱 버전 (예: 0.1.1)'
$tag = Read-Host 'GitHub 릴리스 태그 (예: v0.1.1)'
if ($version -notmatch '^\d+\.\d+\.\d+$' -or $tag -cne "v$version") { throw '버전과 태그를 확인해 주세요.' }
$bundle = Join-Path (Get-Location) 'apps/alpha/desktop/src-tauri/target/release/bundle/nsis'
$installer = "momso_${version}_x64-setup.exe"
if (-not (Test-Path -LiteralPath (Join-Path $bundle $installer) -PathType Leaf)) { throw '설치 파일을 찾지 못했습니다.' }
if (Test-Path -LiteralPath (Join-Path $bundle 'latest.json')) { throw '기존 latest.json을 확인해 주세요.' }
$signature = [IO.File]::ReadAllText((Join-Path $bundle "$installer.sig"))
if ([string]::IsNullOrWhiteSpace($signature)) { throw '서명 파일이 비어 있습니다.' }
$config = Get-Content -Raw -Encoding UTF8 'apps/alpha/desktop/src-tauri/tauri.conf.json' | ConvertFrom-Json
if ($config.version -cne $version) { throw '설정 파일의 버전과 일치하지 않습니다.' }
$endpoint = $config.plugins.updater.endpoints[0]
if ($endpoint -notmatch '/releases/latest/download/latest\.json$') { throw '업데이트 주소를 확인해 주세요.' }
$assetUrl = $endpoint -replace '/releases/latest/download/latest\.json$', "/releases/download/$tag/$installer"
$metadata = @{ version = $version; platforms = @{ 'windows-x86_64' = @{ signature = $signature; url = $assetUrl } } }
$json = $metadata | ConvertTo-Json -Depth 5
[IO.File]::WriteAllText((Join-Path $bundle 'latest.json'), $json, [Text.UTF8Encoding]::new($false))
}
```

4. GitHub Releases에서 해당 태그의 릴리스에 설치 파일, 짝이 되는 `.exe.sig`, `latest.json`과 필요한 제3자 고지를 올린다. GPL 구성요소를 직접 배포하는 경우에는 해당 소스 제공 조건도 충족한다. `latest.json` 안 서명이 `.sig` 파일의 **내용**이고 URL이 실제 설치 파일을 가리키는지 확인한다. 최신 주소는 일반 릴리스를 가리키므로 초안·프리릴리스는 이 주소로 선택되지 않는다. [GitHub 안내](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases)
5. 공개 후 다른 PC나 시험용 계정에 이미 설치된 구버전에서 설정의 업데이트 확인을 누르고 새 버전과 서명 검증을 확인한다. 설치 전 미저장 작업을 저장한다. Windows가 설치 중 앱을 종료하므로 완료 후 다시 켜 버전을 확인한다. 이것은 실제 릴리스 후 사람이 직접 수행할 확인이다.

업데이트 주소를 바꿀 곳은 `src-tauri/tauri.conf.json`의 `plugins.updater.endpoints`다. 위 명령은 이 주소를 읽어 설치 파일 URL도 만든다. [Tauri 공식 업데이트 안내](https://v2.tauri.app/plugin/updater/)
