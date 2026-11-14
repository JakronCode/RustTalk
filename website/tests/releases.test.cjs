'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const html = fs.readFileSync(process.env.RUSTTALK_TEST_HTML || __dirname + '/../index.html', 'utf8');
const source = html.match(/<script id="component-releases">([\s\S]*?)<\/script>/)[1];
const context = { module: { exports: {} }, AbortController, setTimeout, clearTimeout, TextDecoder, Uint8Array };
vm.runInNewContext(source, context);
const api = context.module.exports;
const client = 'rusttalk-windows.exe', linux = 'rusttalk-linux-x86_64.AppImage', server = 'rusttalk-server-linux-x86_64.tar.gz';
function release(tag, names = [client], extra = {}) {
  return { tag_name: tag, draft: false, prerelease: false, assets: names.map(name => ({ name,
    browser_download_url: 'https://github.com/rusttalkcrew/RustTalk/releases/download/' + encodeURIComponent(tag) + '/' + name })), ...extra };
}
function response(items, link = null) { return new Response(JSON.stringify(items), { headers: link ? { link } : {} }); }

test('canonical stable tags, lower hex and u64 fields only; metadata never orders', () => {
  for (const t of ['v0.0.0', 'v18446744073709551615.0.0+client.abcdef1', 'v1.0.0+index-server.abcdef1234']) assert.ok(api.tag(t));
  for (const t of ['v01.0.0', 'v1.00.0', 'v1.0.01', 'v18446744073709551616.0.0', 'v1.0.0+client.ABCDEF1',
    'v1.0.0+client.abcdef', 'v1.0.0+client.' + 'a'.repeat(41), 'v1.0.0-rc.1', 'v1.0.0+other.abcdef1', '1.0.0', 'v1.0.0\n', 'v1.0.0+client.abcdef1\n', '<script>']) assert.equal(api.tag(t), null, t);
});

test('numeric selection, mixed components, legacy preference and eligibility', () => {
  const releases = [release('v9.0.0+server.abcdef1'), release('v99.0.0+index-server.abcdef1'),
    release('v1.9.0'), release('v1.10.0'), release('v1.10.0+client.abcdef1'),
    release('v10.0.0+client.abcdef1', [client], { draft: true }), release('v11.0.0+client.abcdef1', [client], { prerelease: true }),
    release('v2.00.0'), release('v100.0.0-rc.1')];
  assert.equal(api.select(releases, 'client', client).release.tag_name, 'v1.10.0+client.abcdef1');
  assert.equal(api.select([release('v2.0.0')], 'server', client).release.tag_name, 'v2.0.0');
  assert.equal(api.select([release('v99.0.0+server.abcdef1')], 'client', client), null);
});

test('ambiguity refuses before asset filtering; identical numeric metadata does not choose bytes', () => {
  const releases = [release('v1.0.0+client.abcdef1', []), release('v1.0.0+client.abcdef2')];
  assert.throws(() => api.select(releases, 'client', client), /Ambiguous/);
  assert.throws(() => api.history(releases, 'linux'), /Ambiguous/);
});

test('missing exact assets skip newer releases; hostile asset URLs and names never qualify', () => {
  assert.equal(api.select([release('v2.0.0', [])], 'client', client), null);
  assert.equal(api.select([release('v2.0.0', []), release('v1.0.0')], 'client', client).release.tag_name, 'v1.0.0');
  for (const url of ['javascript:alert(1)', 'data:text/html,bad', 'https://evil.example/file',
    'https://github.com/other/RustTalk/releases/download/v1.0.0/' + client,
    'https://github.com/rusttalkcrew/RustTalk/releases/download/v2.0.0/' + client,
    'http://github.com/rusttalkcrew/RustTalk/releases/download/v1.0.0/' + client]) {
    const r = release('v1.0.0'); r.assets[0].browser_download_url = url;
    assert.equal(api.select([r], 'client', client), null);
  }
  const r = release('v1.0.0', ['<img onerror=alert(1)>']);
  assert.equal(api.safeAsset(r.assets[0], r), false);
});

test('history excludes wrong components, empty platforms and checksum-only releases', () => {
  const releases = [release('v8.0.0+server.abcdef1', [server, client]), release('v7.0.0+client.abcdef1', [linux]),
    release('v6.0.0+client.abcdef1', [client + '.sha256']), release('v5.0.0', [client]), release('v4.0.0', [server])];
  assert.deepEqual(Array.from(api.history(releases, 'windows'), r => r.tag_name), ['v5.0.0']);
  assert.deepEqual(Array.from(api.history(releases, 'server'), r => r.tag_name), ['v8.0.0+server.abcdef1', 'v4.0.0']);
});

test('Windows x64 server assets, per-asset legacy fallback and duplicate consistency', () => {
  const winServer = 'rusttalk-server-windows.exe';
  const r = release('v2.0.0+server.abcdef1', [winServer, winServer + '.sha256']);
  assert.equal(api.history([r], 'server')[0].tag_name, r.tag_name);
  const legacy = release('v2.0.0', [client]);
  const partial = release('v2.0.0+client.abcdef1', [linux]);
  for (const list of [[legacy, partial], [partial, legacy]]) {
    assert.equal(api.select(list, 'client', client).release.tag_name, legacy.tag_name);
    assert.equal(api.history(list, 'windows')[0].tag_name, legacy.tag_name);
    assert.equal(api.history(list, 'linux')[0].tag_name, partial.tag_name);
  }
  assert.equal(api.candidates([legacy, legacy], 'client').length, 1);
  assert.throws(() => api.candidates([legacy, release(legacy.tag_name, [])], 'client'), /Inconsistent/);
});

test('pagination uses fixed URLs, never external Link URLs; later failures reject whole listing', async () => {
  const calls = [];
  const result = await api.load(async url => { calls.push(url); return response(calls.length === 1 ? [release('v1.0.0')] : [], '<https://evil.example/>; rel="' + (calls.length === 1 ? 'next' : 'prev') + '"'); });
  assert.equal(result.length, 1);
  assert.deepEqual(calls, [1, 2].map(p => 'https://api.github.com/repos/rusttalkcrew/RustTalk/releases?per_page=100&page=' + p));
  let n = 0;
  await assert.rejects(api.load(async () => ++n === 1 ? response(Array(100).fill(release('v1.0.0'))) : new Response('', { status: 500 })), /failed/);
  n = 0;
  await assert.rejects(api.load(async () => { n++; return response(Array(100).fill(release('v1.0.0'))); }), /Incomplete/);
  assert.equal(n, 10);
});

test('stream byte cap, malformed shape, oversized arrays, broken JSON and invalid UTF-8 refuse', async () => {
  let cancelled = false;
  const body = new ReadableStream({ start(c) { c.enqueue(new Uint8Array(api.LIMIT + 1)); }, cancel() { cancelled = true; } });
  await assert.rejects(api.readPage({ ok: true, body }), /too large/);
  assert.equal(cancelled, true);
  for (const data of [{}, [null], [{ tag_name: 'v1.0.0' }], Array(101).fill(release('v1.0.0'))]) await assert.rejects(api.readPage(response(data)));
  await assert.rejects(api.readPage(new Response('{')));
  await assert.rejects(api.readPage(new Response(new Uint8Array([255]))));
});

class Element {
  constructor(name, text = '') { this.name = name; this.textContent = text; this.children = []; this.dataset = {}; this.events = {}; }
  appendChild(e) { this.children.push(e); }
  addEventListener(event, handler) { this.events[event] = handler; }
  querySelector(selector) { return this.parts[selector]; }
}
function documentFixture(kind = 'windows') {
  const a = new Element('a'); a.href = api.FALLBACK; a.dataset = { component: 'client', asset: client };
  const det = new Element('details'); det.dataset.kind = kind;
  det.parts = { '.ver-sel': new Element('select'), '.list': new Element('div', 'fallback') };
  const label = new Element('span');
  return { a, det, label, documentElement: { dataset: { os: 'windows' } }, createElement: name => new Element(name),
    querySelectorAll: selector => selector === 'a[data-asset]' ? [a] : selector === 'details.files' ? [det] : [label] };
}

test('DOM keeps fallbacks, component/platform labels, history and SHA-256; never inserts HTML', () => {
  const doc = documentFixture(), r = release('v1.2.0+client.abcdef1', [client, client + '.sha256']);
  r.assets[0].digest = 'sha256:' + 'a'.repeat(64);
  api.apply(doc, [release('v99.0.0+server.abcdef1', [server]), r]);
  assert.equal(doc.a.href, r.assets[0].browser_download_url);
  assert.equal(doc.label.textContent, 'Client v1.2.0 · ');
  assert.equal(doc.det.parts['.ver-sel'].children[0].textContent, 'v1.2.0 (latest)');
  const row = doc.det.parts['.list'].children[0].children[0].children[1];
  assert.equal(row.children[1].textContent, 'a'.repeat(64));
  assert.equal(row.children[2].href, r.assets[1].browser_download_url);
  const missing = documentFixture(); api.apply(missing, []);
  assert.equal(missing.a.href, api.FALLBACK);
  assert.equal(missing.det.parts['.list'].textContent, 'fallback');
  assert.equal(source.includes('innerHTML'), false);
  const ambiguous = documentFixture();
  assert.throws(() => api.apply(ambiguous, [release('v1.0.0+server.abcdef1'), release('v1.0.0+server.abcdef2')]), /Ambiguous/);
  assert.equal(ambiguous.a.href, api.FALLBACK);
});
