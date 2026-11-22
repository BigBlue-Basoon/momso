const http = require('node:http')
const { chromium } = require(process.env.MOMSO_S47_PLAYWRIGHT)
const watchdog = setTimeout(() => { console.error('mobile fixture timed out'); process.exit(1) }, 150000)

async function main() {
  let otpAccepted = false
  let secondLoginAccepted = false
  const site = http.createServer((request, response) => {
    let body = ''
    request.on('data', (chunk) => { body += chunk })
    request.on('end', () => {
      if (request.method === 'POST' && request.url === '/verify') otpAccepted = body.includes('code=649217')
      if (request.method === 'POST' && request.url === '/login') secondLoginAccepted = body.includes('username=s47-second') && body.includes('password=s47-fake-second')
      let page = '<html><body>fixture complete</body></html>'
      if (request.method === 'GET' && request.url === '/captcha') page = '<html><body><div class="g-recaptcha">verification required</div></body></html>'
      if (request.method === 'GET' && request.url === '/otp') page = '<html><body><form action="/verify" method="post"><input name="code" autocomplete="one-time-code"></form></body></html>'
      if (request.method === 'GET' && request.url === '/login') page = '<html><body><form action="/login" method="post"><input name="username" autocomplete="username"><input name="password" type="password" autocomplete="current-password"><button>로그인</button></form></body></html>'
      response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', connection: 'close' })
      response.end(page)
    })
  })
  await new Promise((resolve) => site.listen(0, '127.0.0.1', resolve))
  const target = new URL(`http://${process.env.MOMSO_S47_SERVER}`)
  const upstreamRequests = new Set()
  const proxy = http.createServer((request, response) => {
    const headers = { ...request.headers, host: target.host, authorization: 'Bearer test-only-token' }
    const upstream = http.request({ hostname: target.hostname, port: target.port, path: request.url, method: request.method, headers }, (result) => {
      response.writeHead(result.statusCode, result.headers)
      result.pipe(response)
    })
    upstreamRequests.add(upstream)
    upstream.on('close', () => upstreamRequests.delete(upstream))
    response.on('close', () => upstream.destroy())
    upstream.on('error', () => { if (!response.headersSent) response.writeHead(502); response.end() })
    request.pipe(upstream)
  })
  await new Promise((resolve) => proxy.listen(0, '127.0.0.1', resolve))
  const browser = await chromium.launch({ executablePath: process.env.MOMSO_TEST_CHROMIUM, headless: true })
  try {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 1 })
    await page.goto(`http://127.0.0.1:${proxy.address().port}/`, { waitUntil: 'domcontentloaded' })
    const input = page.getByRole('textbox', { name: '메시지 입력' })
    await input.waitFor({ state: 'visible', timeout: 30000 })
    await input.fill(`otp fixture http://127.0.0.1:${site.address().port}/captcha`)
    await page.getByRole('button', { name: '보내기', exact: true }).click()
    await page.getByText('사이트가 본인 확인을 더 요구합니다', { exact: false }).waitFor({ state: 'visible', timeout: 30000 })
    console.log('human-required: visible')
    await page.screenshot({ path: process.env.MOMSO_S47_SCREENSHOT })

    const waitForTurn = async () => {
      await page.waitForFunction(() => {
        const button = document.querySelector('button[aria-label="보내기"]')
        return button && !document.querySelector('button[aria-label="중지"]')
      }, { timeout: 30000 })
    }
    await waitForTurn()
    await input.fill(`otp fixture http://127.0.0.1:${site.address().port}/otp`)
    await page.getByRole('button', { name: '보내기', exact: true }).click()
    const otp = page.getByRole('group', { name: '인증 번호 넣기' })
    await otp.waitFor({ state: 'visible', timeout: 30000 })
    await page.screenshot({ path: `${process.env.MOMSO_S47_SCREENSHOT}.otp.png` })
    await otp.locator('input[data-field="code"]').fill('649217')
    await otp.getByRole('button', { name: '보내기', exact: true }).click()
    for (let i = 0; i < 150 && !otpAccepted; i++) await new Promise((resolve) => setTimeout(resolve, 100))
    if (!otpAccepted) throw new Error('the fake site did not receive the one-time code')
    console.log('one-time-code: visible and delivered')

    const siteUrl = `http://127.0.0.1:${site.address().port}/login`
    for (const [label, username, password] of [['S47 첫째', 's47-first', 's47-fake-first'], ['S47 둘째', 's47-second', 's47-fake-second']]) {
      const saved = await fetch(`http://127.0.0.1:${proxy.address().port}/saved-logins`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ siteUrl, label, username, password }) })
      if (!saved.ok) throw new Error(`fixture login setup failed: ${saved.status}`)
    }
    await waitForTurn()
    await input.fill(`login fixture ${siteUrl}`)
    await page.getByRole('button', { name: '보내기', exact: true }).click()
    const approve = page.getByRole('button', { name: '이번만 허락', exact: true })
    await approve.waitFor({ state: 'visible', timeout: 30000 })
    await approve.click()
    const choice = page.getByRole('group', { name: '저장된 로그인 선택' })
    await choice.waitFor({ state: 'visible', timeout: 30000 })
    await page.screenshot({ path: `${process.env.MOMSO_S47_SCREENSHOT}.choice.png` })
    await choice.getByRole('button', { name: /S47 둘째/ }).click()
    for (let i = 0; i < 150 && !secondLoginAccepted; i++) await new Promise((resolve) => setTimeout(resolve, 100))
    if (!secondLoginAccepted) throw new Error('the fake site did not receive the selected fixture login')
    console.log('saved-login-choice: visible and delivered')
  } finally {
    proxy.closeAllConnections?.()
    site.closeAllConnections?.()
    for (const request of upstreamRequests) request.destroy()
    await browser.close()
    await new Promise((resolve) => proxy.close(resolve))
    await new Promise((resolve) => site.close(resolve))
  }
}
main().then(() => { clearTimeout(watchdog); process.exit(0) }).catch((error) => { clearTimeout(watchdog); console.error(error.message); process.exit(1) })
