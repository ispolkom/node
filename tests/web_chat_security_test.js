const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const html = fs.readFileSync('src/web/ui/chat.html', 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const list = { innerHTML: '', querySelectorAll: () => [] };
const context = {
    document: { addEventListener() {}, getElementById: id => id === 'contactsList' ? list : null },
    window: {}, console, Set, Map, Date, URL, setInterval() {}, setTimeout() {}
};
vm.createContext(context);
vm.runInContext(script, context);
const attack = `'");alert(1);//<img src=x onerror="alert(2)">`;
context.renderContacts([{ id: 'a', short_id: '1234', name: attack, online: false }]);
assert(!list.innerHTML.includes('onclick='));
assert(!list.innerHTML.includes('<img src=x'));
assert(list.innerHTML.includes('&quot;'));
assert(list.innerHTML.includes('&#39;'));
assert.equal(context.escapeHTML(`"'<>&`), '&quot;&#39;&lt;&gt;&amp;');
const attachment = context.renderAttachment({ filename: '" onerror="alert(3)', mime_type: 'image/png', size: 3, data: 'x" onerror="alert(4)' });
assert(!attachment.includes('src="x" onerror='));
assert(!attachment.includes('alt="" onerror='));
console.log('Chat HTML injection tests passed');
