// 개발 서버가 지금 내주는 화면 코드가 디스크의 파일과 같은지 본다(2026-11-21).
//
// 까닭: 한 파일을 아주 짧은 사이에 두 번 쓰면, 개발 서버(vite)가 반쯤 쓴 첫 모양을 읽어 두고 두 번째 쓰기를 놓칠 수 있다.
// 그러면 디스크는 맞는데 실제 앱(새로 불러와도)은 옛 모양을 받는다. 새로 띄운 가짜 화면 서버는 마지막 모양을 읽으므로 가짜 검사로는 잡히지 않는다.
// (f8f989c: PanelBrowser.tsx 의 createPortal 이 둘째 인자 없이 내려가 실제 앱에서만 "Target container is not a DOM element".)
//
// 쓰는 법(apps/alpha/desktop 에서): node tests/ui-check/check-served.mjs [포트=1420] [파일...]
//   파일을 주지 않으면 git 이 바뀌었다고 보는 src 아래 파일 대신, src 아래 모든 .ts/.tsx/.css 를 본다.
// 서버가 내준 모듈의 인라인 소스맵(sourcesContent)을 디스크 내용과 견준다. 다르면 그 파일을 적고 1 로 끝난다.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'

const args = process.argv.slice(2)
const port = /^\d+$/.test(args[0] ?? '') ? args.shift() : '1420'
const root = process.cwd()

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    return statSync(path).isDirectory() ? walk(path) : /\.(tsx?|css)$/.test(name) ? [path] : []
  })
}

const files = (args.length ? args.map((file) => join(root, file)) : walk(join(root, 'src'))).filter((file) => !file.endsWith('.d.ts'))
const normalize = (text) => text.replace(/\r\n/g, '\n')
let stale = 0
let checked = 0
for (const file of files) {
  const url = `http://localhost:${port}/${relative(root, file).split(sep).join('/')}`
  let body
  try {
    const response = await fetch(url)
    if (!response.ok) { console.log(`건너뜀(${response.status}) ${url}`); continue }
    body = await response.text()
  } catch (error) {
    console.log(`서버에 닿지 못함: ${url} (${error.message})`)
    process.exitCode = 2
    break
  }
  const disk = normalize(readFileSync(file, 'utf8'))
  let served = null
  const map = body.match(/sourceMappingURL=data:application\/json;(?:charset=utf-8;)?base64,([A-Za-z0-9+/=]+)/)
  if (map) {
    try { served = JSON.parse(Buffer.from(map[1], 'base64').toString('utf8')).sourcesContent?.[0] ?? null } catch { served = null }
  }
  if (served === null) {
    // CSS 는 소스맵 없이 글로 내려온다. @import 를 품은 것(내려올 때 펼쳐짐)과 .module.css(이름이 바뀜)는 견줄 수 없다.
    if (file.endsWith('.module.css') || /@import\b/.test(disk)) { console.log(`견줄 수 없음(펼쳐지거나 이름이 바뀌는 CSS) ${relative(root, file)}`); continue }
    if (file.endsWith('.css')) {
      const text = body.match(/const __vite__css = "([\s\S]*?)"\n/)?.[1]
      served = text ? JSON.parse(`"${text}"`) : null
    }
  }
  if (served === null) { console.log(`견줄 수 없음 ${url}`); continue }
  checked += 1
  if (normalize(served) !== disk) {
    stale += 1
    console.log(`옛 모양을 내주고 있음: ${relative(root, file)}`)
  }
}
console.log(`${checked}개 견줌, 다른 것 ${stale}개`)
// process.exit 는 Windows 에서 열린 fetch 연결 때문에 libuv 단정 오류로 끝날 수 있어, 끝날 값만 정한다.
if (stale) process.exitCode = 1
