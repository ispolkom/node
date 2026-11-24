// Exercise the actual phone signaling state machine with controlled network/timers.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../src/web/ui/phone-call.js'), 'utf8');
function boot() {
  const sent = [], intervals = new Map(), elements = new Map();
  let id = 0, rings = 0;
  const window = {addEventListener() {}, isSoundEnabled: () => true};
  const context = vm.createContext({window, console, Uint8Array, Date, Map,
    crypto: {getRandomValues(a) { a.fill(42); }},
    document: {hidden: false, getElementById(key) {
      if (!elements.has(key)) elements.set(key, {style: {}, remove() {}});
      return elements.get(key);
    }},
    Audio: class {play() { rings++; return Promise.resolve(); } pause() {}},
    setInterval(fn, ms) { intervals.set(++id, {fn, ms}); return id; },
    clearInterval(key) { intervals.delete(key); },
    setTimeout() { return ++id; }, clearTimeout() {},
    fetch(url, opts) {
      if (url === '/api/mobile/call/signal') sent.push(JSON.parse(JSON.parse(opts.body).text.slice(12)));
      return Promise.resolve({json: async () => ({status: 'ok', last: 0, events: []})});
    },
  });
  const exported = 'window.phoneCall = { start: start };';
  assert(source.includes(exported));
  vm.runInContext(source.replace(exported,
    'window.phoneCall = {start: start, onEvent: onEvent, hangup: hangup, current: function () {return call;}};'), context);
  return {sent, api: window.phoneCall, rings: () => rings,
    tick() { for (const timer of [...intervals.values()]) if (timer.ms === 2000) timer.fn(); },
    event(t, cid, peer = 'phone', video = false) {
      window.phoneCall.onEvent({from: peer, name: 'Phone', text: '\u0001yandi-call:' + JSON.stringify({v: 1, t, cid, video, ts: Date.now()})});
    }};
}
async function flush() { for (let i = 0; i < 8; i++) await Promise.resolve(); }
(async () => {
  let b = boot();
  b.api.start('phone', 'Phone', true);
  const cid = b.api.current().cid;
  b.tick(); await flush();
  assert.equal(b.sent.length, 2);
  assert(b.sent.every(s => s.t === 'invite' && s.cid === cid && s.video));
  b.event('ringing', 'wrongcid', 'phone', true);
  b.tick(); await flush(); assert.equal(b.sent.length, 3);
  b.event('ringing', cid, 'phone', true);
  b.tick(); await flush(); assert.equal(b.sent.length, 3);
  b.api.hangup();
  b.tick(); await flush(); assert.equal(b.sent.at(-1).t, 'hangup');

  b = boot();
  b.event('invite', 'call0001');
  b.event('invite', 'call0001');
  assert.deepEqual(b.sent.map(s => s.t), ['ringing', 'ringing']);
  assert.equal(b.rings(), 1);
  b.api.hangup();
  b.event('invite', 'call0001');
  assert.equal(b.rings(), 1, 'late retransmission must not reopen canceled call');

  b = boot();
  b.event('hangup', 'call0001'); b.event('invite', 'call0001');
  assert.equal(b.api.current(), null);
  assert.equal(b.rings(), 0);
  b.event('invite', 'call0002');
  assert.equal(b.rings(), 1, 'fresh call is still accepted');

  b = boot(); b.api.start('phone', 'Phone', false);
  for (let i = 0; i < 15; i++) { b.tick(); await flush(); }
  assert.equal(b.sent.length, 11, 'initial invite plus ten bounded retries');
  console.log('Phone call receipts, bounded retries, duplicate and canceled invites: OK');
})().catch(e => {console.error(e); process.exitCode = 1;});
