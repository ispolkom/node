// Browser audio regression checks without a browser, network, or real audio.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.resolve(__dirname, '..');
const ui = path.join(root, 'src/web/ui');
const created = [], listeners = new Map(), settings = new Map();
class Audio {
  constructor(src) { this.src = this.originalSrc = src; this.paused = true; created.push(this); }
  play() { this.paused = false; return Promise.resolve(); }
  pause() { this.paused = true; }
}
const window = {
  addEventListener(type, callback) {
    if (!listeners.has(type)) listeners.set(type, []);
    listeners.get(type).push(callback);
  },
  dispatchEvent(event) { for (const callback of listeners.get(event.type) || []) callback(event); },
};
const context = vm.createContext({window, Audio, console,
  document: {readyState: 'loading', addEventListener() {}, getElementById() { return null; }, querySelectorAll() { return []; }},
  localStorage: {getItem: (key) => settings.get(key) ?? null, setItem: (key, value) => settings.set(key, value)},
  Event: class { constructor(type) { this.type = type; } },
  setTimeout() { return 1; }, clearTimeout() {}, setInterval() { return 1; }, clearInterval() {},
});
const source = (name) => fs.readFileSync(path.join(ui, name), 'utf8');
const app = source('app.js');
vm.runInContext(app.slice(app.indexOf('// === Sound System'), app.indexOf('// === Apply translations')), context);
assert.equal(window.isSoundEnabled(), true, 'sounds are enabled on first launch');
window.playMessageSound();
assert.equal(created.find((a) => !a.paused).originalSrc, '/media/icq.mp3');
window.playGroupSound();
assert.equal(created.at(-1).originalSrc, '/media/icq-online.mp3');

for (const [name, type, incoming] of [['voice-call.js', 'VoiceCallManager', 'calling.mp3'], ['video-call.js', 'VideoCallManager', 'sqype.mp3']]) {
  vm.runInContext(source(name), context);
  const manager = vm.runInContext(`new ${type}()`, context);
  manager._playRingtone('calling');
  const tone = created.at(-1);
  assert.equal(tone.originalSrc, '/media/' + incoming);
  assert.equal(tone.loop, true);
  window.toggleSound();
  assert.equal(tone.paused, true, 'master bell immediately stops a playing ring');
  window.toggleSound();
  assert.equal(created.at(-1).originalSrc, '/media/' + incoming, 'unmute resumes a still-pending call');
  manager._stopRingtones();
  const count = created.length;
  window.toggleSound(); window.toggleSound();
  assert.equal(created.length, count, 'accept/end clears the ring intent');
  manager._playRingtone('call_out');
  assert.equal(created.at(-1).originalSrc, '/media/call_out.mp3');
  manager._stopRingtones();
}

for (const name of ['app.js', 'voice-call.js', 'video-call.js', 'phone-call.js']) new vm.Script(source(name));
const phone = source('phone-call.js');
const tones = phone.slice(phone.indexOf('    var ringAudio = null;'), phone.indexOf('    // ── окно звонка'));
vm.runInContext('var call = null;\n' + tones, context);
for (const [outgoing, video, expected] of [[false, false, 'calling.mp3'], [false, true, 'sqype.mp3'], [true, false, 'call_out.mp3'], [true, true, 'call_out.mp3']]) {
  context.call = {video, phase: outgoing ? 'calling' : 'ringing'};
  context.ringStart(outgoing);
  assert.equal(created.at(-1).originalSrc, '/media/' + expected);
  context.ringStop();
}

for (const name of ['icq.mp3', 'icq-online.mp3', 'calling.mp3', 'sqype.mp3', 'call_out.mp3']) {
  const original = fs.readFileSync(path.join(ui, 'media', name));
  assert(original.length > 1000);
  for (const client of ['mobile/yandi_mobile', 'desktop/yandi_desktop']) {
    assert(original.equals(fs.readFileSync(path.join(root, client, 'assets/sounds', name))), `${client}: ${name} differs from the web tone`);
  }
}
console.log('Web/mobile/desktop tone assets, default-on sound, video/audio/dial mapping and master mute: OK');
