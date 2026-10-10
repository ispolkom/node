const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const html = fs.readFileSync('src/web/ui/chat.html', 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const list = { innerHTML: '', querySelectorAll: () => [] };
const other = { innerHTML: '' };
const attack = `'");alert(1);//<img src=x onerror="alert(2)">`;
// страница при загрузке опрашивает узел: отвечаем подставными данными (имя устройства — с попыткой внедрения)
const responses = {
    '/api/mobile/other-devices': { devices: [{ peer_id: 'a'.repeat(64), name: attack, online: true, status: 'new' }] },
    '/api/groups': { groups: [] },
};
const fetch = url => Promise.resolve({ ok: true, json: () => Promise.resolve(responses[url.split('?')[0]] || {}) });
const context = {
    document: { addEventListener() {}, getElementById: id => id === 'contactsList' ? list : (id === 'otherDevices' ? other : null) },
    window: {}, console, Set, Map, Date, URL, setInterval() {}, setTimeout() {}, fetch, localStorage: { getItem() { return null; }, setItem() {} },
    JSON, Promise, Object, Array, String, RegExp,
};
vm.createContext(context);
vm.runInContext(script, context);
context.renderContacts([{ id: 'a', short_id: '1234', name: attack, online: false }]);
assert(!list.innerHTML.includes('onclick='));
assert(!list.innerHTML.includes('<img src=x'));
assert(list.innerHTML.includes('&quot;'));
assert(list.innerHTML.includes('&#39;'));
assert.equal(context.escapeHTML(`"'<>&`), '&quot;&#39;&lt;&gt;&amp;');
const attachment = context.renderAttachment({ filename: '" onerror="alert(3)', mime_type: 'image/png', size: 3, data: 'x" onerror="alert(4)' });
assert(!attachment.includes('src="x" onerror='));
assert(!attachment.includes('alt="" onerror='));
// приглашение в группу: только корректный номер группы и код, имя без управляющих символов
assert.equal(context.groupInviteOf('\u0001yandi-gi:{"g":"zz","c":"AAAA-BBBB-CCCC","n":"x"}'), null, 'неверный номер группы');
assert.equal(context.groupInviteOf('\u0001yandi-gi:{"g":"' + 'a'.repeat(32) + '","c":"<b>","n":"x"}'), null, 'неверный код');
const gi = context.groupInviteOf('\u0001yandi-gi:{"g":"' + 'A'.repeat(32) + '","c":"","n":"Имя\\u0007<i>"}');
assert.equal(gi.g, 'a'.repeat(32));
assert(!gi.n.includes('\u0007'));
// «Другие устройства узла»: имя устройства экранируется
setImmediate(() => {
    assert(other.innerHTML.length > 0, 'список других устройств отрисован');
    assert(!other.innerHTML.includes('<img src=x'), 'имя устройства не стало разметкой');
    assert(other.innerHTML.includes('&lt;img'));
    console.log('Chat HTML injection tests passed');
});
