// Contract examples only. This fake driver and approval gate are not product integration.
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const services = ['naver-smartplace', 'onstudio', 'studiomate', 'soomgo', 'frip', 'sssd'];
const digest = (v) => createHash('sha256').update(JSON.stringify(v)).digest('hex');

async function fakeSite() {
  let writes = 0;
  const server = createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/screen') {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      return res.end('<!doctype html><html lang="ko"><title>가짜 운영 화면</title><main><h1>가짜 수업</h1><p>오전 10시</p><form method="post" action="/change"><button>변경</button></form></main></html>');
    }
    if (req.method === 'POST' && req.url === '/change') { writes++; res.writeHead(204); return res.end(); }
    res.writeHead(404); res.end();
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { url: `http://127.0.0.1:${server.address().port}`, writes: () => writes,
    close: () => new Promise((resolve) => server.close(resolve)) };
}

async function fakeDriver(site, preview, { approval, approvalAvailable = true, changed, screenAvailable = true, outcomeUnknown = false }) {
  if (!approvalAvailable || !screenAvailable) return 'handoff';
  const page = await (await fetch(`${site.url}/screen`)).text();
  if (!page.includes('가짜 수업')) return 'handoff';
  if (!approval || approval.used || approval.digest !== digest(preview)) return 'denied';
  if (changed && digest(changed) !== approval.digest) return 'changed';
  approval.used = true;
  const response = await fetch(`${site.url}/change`, { method: 'POST' });
  if (outcomeUnknown) return 'unknown'; // Never resend an uncertain change.
  return response.status === 204 ? 'confirmed' : 'unknown';
}

test('read-only reads the fake site and refuses a change without approval', async () => {
  const site = await fakeSite();
  try {
    let approvalRequests = 0;
    const page = await (await fetch(`${site.url}/screen`)).text();
    assert.match(page, /가짜 수업/);
    const method = 'browser_click';
    const allowed = new Set(['browser_navigate', 'browser_read']);
    if (allowed.has(method)) approvalRequests++;
    assert.equal(allowed.has(method), false);
    assert.equal(approvalRequests, 0);
    assert.equal(site.writes(), 0);
  } finally { await site.close(); }
});

for (const service of services) {
  test(`${service}: approval is single-use and bound to the exact change`, async () => {
    const site = await fakeSite();
    try {
      const preview = { service, account: 'fixture-account', target: 'fixture-class', before: '10:00', after: '11:00', notification: null, cost: null, irreversible: true };
      const approved = { digest: digest(preview), used: false };
      assert.equal(await fakeDriver(site, preview, { approval: null }), 'denied');
      assert.equal(await fakeDriver(site, preview, { approval: approved, approvalAvailable: false }), 'handoff');
      assert.equal(await fakeDriver(site, preview, { approval: approved, screenAvailable: false }), 'handoff');
      assert.equal(await fakeDriver(site, preview, { approval: approved, changed: { ...preview, account: 'different-account' } }), 'changed');
      assert.equal(site.writes(), 0);
      assert.equal(await fakeDriver(site, preview, { approval: approved }), 'confirmed');
      assert.equal(site.writes(), 1);
      assert.equal(await fakeDriver(site, preview, { approval: approved }), 'denied');
      assert.equal(await fakeDriver(site, preview, { approval: { digest: digest(preview), used: false }, outcomeUnknown: true }), 'unknown');
      assert.equal(site.writes(), 2);
    } finally { await site.close(); }
  });
}
