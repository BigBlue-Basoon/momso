import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { webcrypto } from 'node:crypto';
import http from 'node:http';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import test from 'node:test';
import vm from 'node:vm';
import { bootLoginPage } from './momso_anywhere_login_boot.mjs';

const fakeServer = String.raw`
import base64,json,os,sys,http.server
sys.path.insert(0,'scripts')
import momso_anywhere_auth as auth
import momso_anywhere_gateway as gateway
os.environ.update(MOMSO_PUBLIC_HOST='fixture.run.app',MOMSO_LOGIN_ORIGIN='https://login.example.test',MOMSO_GOOGLE_CLIENT_ID='fixture-client',MOMSO_OWNER_SUB='fixture-owner')
auth._key=lambda: b'x'*64
original=auth.finish
def verifier(token,client):
 value=token.split('.')[1]
 return json.loads(base64.urlsafe_b64decode(value+'='*(-len(value)%4)))
auth.finish=lambda state,cookie,token,origin:original(state,cookie,token,origin,verifier=verifier)
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),gateway.Handler)
print(server.server_address[1],flush=True)
server.serve_forever()
`;

function request(port, method, path, headers = {}, body = undefined) {
  return new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port, method, path, headers: { Host: 'fixture.run.app', ...headers } }, res => {
      const chunks = [];
      res.on('data', part => chunks.push(part));
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks).toString() }));
    });
    req.on('error', reject);
    req.end(body);
  });
}

test('fake Google redirect completes gateway session without exposing fragment to HTTP', async t => {
  const child = spawn(process.platform === 'win32' ? 'python' : 'python3', ['-u', '-c', fakeServer], { cwd: process.cwd(), stdio: ['ignore', 'pipe', 'pipe'] });
  t.after(() => child.kill());
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([once(lines, 'line'), once(child, 'exit').then(() => { throw new Error('fixture failed'); })]);
  const port = Number(line);
  assert(port > 0);
  const first = await request(port, 'GET', '/', { 'Sec-Fetch-Mode': 'navigate', 'Sec-Fetch-Dest': 'document' });
  assert.equal(first.status, 200);
  assert.equal(first.headers['referrer-policy'], 'strict-origin');
  assert(first.body.includes('http-equiv="refresh"'));
  const redirectScript = first.body.match(/<script>([\s\S]*?)<\/script>/)[1];
  const intermediate = { replace(value) { this.next = value; } };
  vm.runInNewContext(redirectScript, { location: intermediate });
  const login = new URL(intermediate.next);
  const signedState = login.searchParams.get('state');
  const challengeCookie = first.headers['set-cookie'][0].split(';')[0];
  const values = new Map();
  const storage = { getItem: k => values.get(k) ?? null, setItem: (k, v) => values.set(k, v), removeItem: k => values.delete(k) };
  const button = { addEventListener(_name, fn) { this.click = fn; }, disabled: true };
  const doc = { body: { dataset: {} }, referrer: 'https://fixture.run.app/', getElementById: () => button };
  const win = { location: { href: login.href, origin: login.origin, assign(v) { this.assigned = v; }, replace(v) { this.replaced = v; } }, history: { replaceState() {} }, sessionStorage: storage };
  await bootLoginPage({ doc, win, fetchImpl: async () => ({ ok: true, json: async () => ({ clientId: 'fixture-client' }) }), cryptoImpl: webcrypto });
  assert.equal(doc.body.dataset.state, 'ready');
  await button.click();
  const authorization = new URL(win.location.assigned);
  const token = `a.${Buffer.from(JSON.stringify({ sub: 'fixture-owner', nonce: authorization.searchParams.get('nonce') })).toString('base64url')}.c`;
  const googleReturn = `${login.origin}/#id_token=${token}&state=${authorization.searchParams.get('state')}`;
  win.location.href = googleReturn;
  doc.referrer = 'https://accounts.google.com/';
  let cleared = false;
  win.history.replaceState = () => { cleared = true; };
  await bootLoginPage({ doc, win, fetchImpl: async () => assert.fail('no request'), cryptoImpl: webcrypto });
  assert(cleared);
  const callback = new URL(win.location.replaced);
  assert.equal(callback.pathname, '/__momso/auth/return');
  assert.equal(callback.search, '');
  const page = await request(port, 'GET', callback.pathname, { Cookie: challengeCookie });
  assert.equal(page.status, 200);
  assert(!page.body.includes(token));
  const script = page.body.match(/<script>([\s\S]*)<\/script>/)[1];
  const history = [];
  const location = { hash: callback.hash, pathname: callback.pathname, replace(value) { this.next = value; } };
  let cookie;
  const scriptResult = vm.runInNewContext(script, {
    location, URLSearchParams, history: { replaceState(_a, _b, path) { history.push(path); } },
    fetch: async (path, options) => {
      assert.deepEqual(history, ['/__momso/auth/return']);
      const body = JSON.parse(options.body);
      assert.equal(body.state, signedState);
      const reply = await request(port, 'POST', path, { Origin: 'https://fixture.run.app', Cookie: challengeCookie, 'Content-Type': 'application/json' }, options.body);
      cookie = reply.headers['set-cookie']?.[0];
      return { ok: reply.status === 200 };
    },
  });
  await scriptResult;
  assert.equal(location.next, '/');
  assert(cookie.includes('HttpOnly; SameSite=Strict'));
});
