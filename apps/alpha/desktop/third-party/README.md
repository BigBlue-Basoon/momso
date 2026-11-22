# 제3자 프로그램 고지

- Windows 설치 파일에는 Microsoft Visual C++ 재배포 가능 런타임 DLL이 포함된다. [Microsoft 재배포 안내](https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files)와 [Visual Studio 2022 재배포 대상](https://learn.microsoft.com/visualstudio/releases/2022/redistribution)을 따른다.

- 이 앱에는 Git for Windows의 MinGit 2.56.1이 포함된다. 라이선스 원문은 같은 폴더의 `MinGit-LICENSE.txt`이고, MinGit에 포함된 구성 요소별 라이선스 파일은 설치된 `mingit/` 안에도 있다.
- 고정된 공식 배포본: https://github.com/git-for-windows/git/releases/tag/v2.56.1.windows.1
- 해당 Git 소스 패키지: https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/mingw-w64-git-2.56.1.1-1.src.tar.gz
- MinGit 패키지와 버전 목록: https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/package-versions-2.56.1-MinGit.txt
- Git for Windows 빌드 도구: https://github.com/git-for-windows/build-extra
- MinGit에는 여러 패키지가 포함된다. 위 Git 소스 하나가 모든 구성 요소의 대응 소스라는 뜻은 아니다. 배포 전 각 패키지의 대응 소스와 빌드·설치 스크립트를 확보해 제공해야 한다.
- `dependency-licenses.json`은 Cargo와 npm의 공식 명령으로 뽑은 의존성 이름·버전·라이선스 목록이다. Cargo는 모든 대상과 빌드 의존성을 포함하고 npm은 다른 작업 공간의 의존성도 포함할 수 있다. `license: null`은 확인되지 않은 값이다. 목록은 각 의존성의 라이선스 원문과 요구 사항을 대신하지 않는다.
- 이 목록은 저장소의 Cargo·npm 잠금 파일만 대상으로 하므로, 나중에 별도로 설치하거나 묶는 Node.js와 AI CLI의 라이선스는 따로 확인해야 한다.
- 다시 만들기: 저장소 맨 위에서 `node apps/alpha/desktop/scripts/generate-third-party-licenses.mjs`. Cargo 의존성이 로컬에 준비되어 있어야 한다.
- 통합 브랜치에서 별도로 실행한 `npm ls --package-lock-only --workspace @momso/alpha-desktop --all`은 `@rolldown/pluginutils@1.0.0-rc.7` 버전 충돌을 보고했다. 따라서 npm 잠금 파일에 기록된 목록을 생성했지만 의존성 트리의 정상 여부는 미확인이다. 화면 담당 폴더의 같은 검사는 통과했다.
