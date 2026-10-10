// Звонки и видеозвонки телефон ↔ эта страница (компьютер владельца как собеседник телефона).
//
// Сигналы — тот же протокол, что у приложения (call_signal.dart): '\u0001yandi-call:' + {v, t, cid, video, ts, sdp?, cand?};
// t = invite | accept | reject | busy | hangup | offer | answer | ice. Узел шифрует их для телефона и отдаёт нам расшифрованные
// (страница опрашивает /api/mobile/call/events раз в секунду; пока опрашивает — узел не отвечает телефону «не на связи»).
// Звук и видео идут напрямую браузер ↔ телефон (WebRTC, DTLS-SRTP), узел их не видит. Кто звонит, тот после «accept» шлёт offer.
// Нет микрофона или камеры — звонок всё равно принимается: тогда только слышим/видим собеседника.
(function () {
    var MARK = '\u0001yandi-call:';
    var after = null;   // номер последнего события очереди; null — ещё не знаем (старые события при загрузке страницы не проигрываем)
    var call = null;    // {peer, name, cid, video, incoming, phase, pc, local, pendingIce, remoteSet, timers}

    function el(id) { return document.getElementById(id); }
    function esc(t) { return String(t == null ? '' : t).replace(/&/g, '&amp;').replace(/</g, '&lt;'); }
    function newCid() { var a = new Uint8Array(8); crypto.getRandomValues(a); return Array.from(a, function (b) { return ('0' + b.toString(16)).slice(-2); }).join(''); }

    function send(peer, msg) {
        return fetch('/api/mobile/call/signal', {
            method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ peer: peer, text: MARK + JSON.stringify(msg) })
        }).then(function (r) { return r.json(); }).then(function (j) { if (j.status !== 'ok') throw new Error(j.message || 'нет связи'); });
    }
    function sig(t, extra) {
        if (!call) return Promise.resolve();
        var m = { v: 1, t: t, cid: call.cid, video: call.video, ts: Date.now() };
        if (extra) for (var k in extra) m[k] = extra[k];
        return send(call.peer, m);
    }

    // ── звонок (без файлов: простой сигнал WebAudio) ──
    var ringCtx = null, ringTimer = null;
    function ringStart(outgoing) {
        ringStop();
        try {
            ringCtx = new (window.AudioContext || window.webkitAudioContext)();
            var beep = function () {
                if (!ringCtx) return;
                var o = ringCtx.createOscillator(), g = ringCtx.createGain();
                o.frequency.value = outgoing ? 425 : 660; g.gain.value = 0.08;
                o.connect(g); g.connect(ringCtx.destination); o.start(); o.stop(ringCtx.currentTime + (outgoing ? 1.0 : 0.4));
            };
            beep(); ringTimer = setInterval(beep, outgoing ? 4000 : 1200);
        } catch (e) {}
    }
    function ringStop() { if (ringTimer) clearInterval(ringTimer); ringTimer = null; if (ringCtx) { try { ringCtx.close(); } catch (e) {} } ringCtx = null; }

    // ── окно звонка ──
    function overlay() {
        var o = el('phoneCallOverlay');
        if (o) return o;
        o = document.createElement('div');
        o.id = 'phoneCallOverlay';
        o.style.cssText = 'position:fixed;inset:0;background:rgba(0,0,0,.85);z-index:10000;display:flex;flex-direction:column;align-items:center;justify-content:center;color:#fff;gap:12px;font-family:inherit';
        o.innerHTML = '<div id="pcTitle" style="font-size:22px;font-weight:bold"></div><div id="pcStatus" style="opacity:.8"></div>'
            + '<div style="position:relative"><video id="pcRemote" autoplay playsinline style="max-width:80vw;max-height:60vh;background:#000;border-radius:8px;display:none"></video>'
            + '<video id="pcLocal" autoplay playsinline muted style="position:absolute;right:8px;bottom:8px;width:160px;border-radius:6px;display:none"></video></div>'
            + '<audio id="pcAudio" autoplay></audio><div id="pcNote" style="opacity:.7;font-size:13px"></div><div id="pcButtons" style="display:flex;gap:10px"></div>';
        document.body.appendChild(o);
        return o;
    }
    function render() {
        if (!call) { var o = el('phoneCallOverlay'); if (o) o.remove(); return; }
        overlay();
        el('pcTitle').textContent = (call.video ? '🎥 ' : '📞 ') + call.name;
        var status = { ringing: call.video ? 'Входящий видеозвонок' : 'Входящий звонок', calling: 'Вызов…', connecting: 'Соединение…', active: 'Разговор', ended: call.endReason || 'Звонок завершён' }[call.phase] || '';
        if (call.phase === 'active' && call.started) {
            var s = Math.floor((Date.now() - call.started) / 1000);
            status += ' · ' + ('0' + Math.floor(s / 60)).slice(-2) + ':' + ('0' + s % 60).slice(-2);
        }
        el('pcStatus').textContent = status;
        var b = '';
        if (call.phase === 'ringing') b = '<button class="btn btn-success" id="pcAccept">✅ Принять</button><button class="btn btn-danger" id="pcReject">❌ Отклонить</button>';
        else if (call.phase !== 'ended') b = (call.local && call.local.getAudioTracks().length ? '<button class="btn" id="pcMute">' + (call.muted ? '🎤 Включить микрофон' : '🔇 Выключить микрофон') + '</button>' : '') + '<button class="btn btn-danger" id="pcHangup">📕 Завершить</button>';
        el('pcButtons').innerHTML = b;
        if (el('pcAccept')) el('pcAccept').onclick = accept;
        if (el('pcReject')) el('pcReject').onclick = reject;
        if (el('pcHangup')) el('pcHangup').onclick = hangup;
        if (el('pcMute')) el('pcMute').onclick = toggleMute;
    }
    setInterval(function () { if (call && call.phase === 'active') render(); }, 1000);

    // ── WebRTC ──
    function iceConfig() {
        return fetch('/api/mobile/call/turn', { cache: 'no-store' }).then(function (r) { return r.json(); }).then(function (j) {
            var t = j.turn, servers = [];
            if (t) servers.push({ urls: ['turn:' + t.host + ':' + t.port + '?transport=udp', 'turn:' + t.host + ':' + t.port + '?transport=tcp'], username: t.username, credential: t.credential });
            return { iceServers: servers, bundlePolicy: 'max-bundle' };
        }).catch(function () { return { iceServers: [] }; });
    }

    function getMedia(video) {
        var md = navigator.mediaDevices;
        if (!md || !md.getUserMedia) return Promise.resolve(null);
        return md.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true }, video: video ? { width: { ideal: 640 }, height: { ideal: 480 } } : false })
            .catch(function () { return video ? md.getUserMedia({ audio: true, video: false }).catch(function () { return null; }) : null; })
            .catch(function () { return null; });
    }

    function startMedia(offerer) {
        var c = call;
        return Promise.all([iceConfig(), getMedia(c.video)]).then(function (r) {
            if (call !== c) return;
            var pc = new RTCPeerConnection(r[0]);
            c.pc = pc; c.local = r[1];
            pc.onicecandidate = function (e) { if (e.candidate) sig('ice', { cand: { candidate: e.candidate.candidate, sdpMid: e.candidate.sdpMid, sdpMLineIndex: e.candidate.sdpMLineIndex } }).catch(function () {}); };
            pc.ontrack = function (e) {
                var st = e.streams && e.streams[0];
                if (!st) return;
                if (e.track.kind === 'video') { el('pcRemote').srcObject = st; el('pcRemote').style.display = ''; }
                else el('pcAudio').srcObject = st;
            };
            pc.onconnectionstatechange = function () {
                if (call !== c) return;
                if (pc.connectionState === 'connected' && c.phase !== 'active') { c.phase = 'active'; c.started = Date.now(); render(); }
                else if (pc.connectionState === 'failed') finish('Не удалось соединиться');
                else if (pc.connectionState === 'disconnected') { clearTimeout(c.lost); c.lost = setTimeout(function () { if (call === c && pc.connectionState !== 'connected') finish('Связь потеряна'); }, 10000); }
            };
            if (c.local) {
                c.local.getTracks().forEach(function (t) { pc.addTrack(t, c.local); });
                if (c.local.getVideoTracks().length) { el('pcLocal').srcObject = c.local; el('pcLocal').style.display = ''; }
            }
            var noMic = !c.local || !c.local.getAudioTracks().length, noCam = c.video && (!c.local || !c.local.getVideoTracks().length);
            el('pcNote').textContent = noMic ? 'На этом компьютере нет микрофона: вы только слышите' + (c.video ? ' и видите' : '') + ' собеседника' : (noCam ? 'Камеры нет: собеседник вас только слышит' : '');
            render();
            if (offerer) {
                if (noMic) pc.addTransceiver('audio', { direction: 'recvonly' });
                if (c.video && noCam) pc.addTransceiver('video', { direction: 'recvonly' });
                return pc.createOffer({ offerToReceiveAudio: true, offerToReceiveVideo: c.video }).then(function (o) {
                    return pc.setLocalDescription(o).then(function () { return sig('offer', { sdp: o.sdp }); });
                });
            }
            if (c.pendingOffer) { var sdp = c.pendingOffer; c.pendingOffer = null; return onOffer(sdp); }
        }).catch(function (e) { finish('Не удалось начать звонок: ' + (e && e.message ? e.message : e)); });
    }

    function remoteReady() {
        call.remoteSet = true;
        var p = Promise.resolve();
        (call.pendingIce || []).forEach(function (c) { p = p.then(function () { return call.pc.addIceCandidate(c).catch(function () {}); }); });
        call.pendingIce = [];
        return p;
    }
    function onOffer(sdp) {
        var c = call;
        if (!c.pc) { c.pendingOffer = sdp; return Promise.resolve(); } // ещё готовим микрофон
        return c.pc.setRemoteDescription({ type: 'offer', sdp: sdp }).then(remoteReady)
            .then(function () { return c.pc.createAnswer(); })
            .then(function (a) { return c.pc.setLocalDescription(a).then(function () { return sig('answer', { sdp: a.sdp }); }); })
            .catch(function (e) { finish('Не удалось ответить: ' + (e && e.message ? e.message : e)); });
    }
    function onIce(cand) {
        var c = new RTCIceCandidate({ candidate: cand.candidate, sdpMid: cand.sdpMid, sdpMLineIndex: cand.sdpMLineIndex });
        if (call.pc && call.remoteSet) call.pc.addIceCandidate(c).catch(function () {});
        else (call.pendingIce = call.pendingIce || []).push(c);
    }

    // ── действия ──
    function accept() {
        if (!call || call.phase !== 'ringing') return;
        ringStop(); clearTimeout(call.ringTimeout);
        call.phase = 'connecting'; render();
        sig('accept').then(function () { return startMedia(false); }).catch(function (e) { finish('Нет связи с телефоном: ' + e.message); });
    }
    function reject() { if (!call) return; sig('reject').catch(function () {}); finish('Звонок отклонён'); }
    function hangup() { if (!call) return; sig('hangup').catch(function () {}); finish(call.phase === 'calling' ? 'Звонок отменён' : 'Звонок завершён'); }
    function toggleMute() {
        if (!call || !call.local) return;
        call.muted = !call.muted;
        call.local.getAudioTracks().forEach(function (t) { t.enabled = !call.muted; });
        render();
    }
    function finish(reason) {
        var c = call;
        if (!c || c.phase === 'ended') return;
        ringStop(); clearTimeout(c.ringTimeout); clearTimeout(c.lost);
        try { if (c.local) c.local.getTracks().forEach(function (t) { t.stop(); }); } catch (e) {}
        try { if (c.pc) c.pc.close(); } catch (e) {}
        c.phase = 'ended'; c.endReason = reason; render();
        setTimeout(function () { if (call === c) { call = null; render(); } }, 2000);
    }

    function start(peer, name, video) {
        if (call) { alert('Вы уже разговариваете'); return; }
        call = { peer: peer, name: name || 'Телефон', cid: newCid(), video: !!video, incoming: false, phase: 'calling', pendingIce: [] };
        render(); ringStart(true);
        sig('invite').catch(function (e) { finish('Не дозвониться: ' + e.message); });
        call.ringTimeout = setTimeout(function () { if (call && call.phase === 'calling') { sig('hangup').catch(function () {}); finish('Нет ответа'); } }, 45000);
    }

    function onEvent(e) {
        if (!e.text || e.text.indexOf(MARK) !== 0) return;
        var s;
        try { s = JSON.parse(e.text.slice(MARK.length)); } catch (x) { return; }
        if (!s || !s.cid || Math.abs(Date.now() - (s.ts || 0)) > 90000) return; // устаревший сигнал звонить не должен
        var mine = call && call.peer === e.from && call.cid === s.cid && call.phase !== 'ended';
        switch (s.t) {
            case 'invite':
                if (call) { if (!mine) send(e.from, { v: 1, t: 'busy', cid: s.cid, video: !!s.video, ts: Date.now() }).catch(function () {}); return; }
                call = { peer: e.from, name: e.name, cid: s.cid, video: !!s.video, incoming: true, phase: 'ringing', pendingIce: [] };
                render(); ringStart(false);
                if (window.Notification && Notification.permission === 'granted' && document.hidden) new Notification((s.video ? 'Видеозвонок' : 'Звонок') + ': ' + e.name);
                call.ringTimeout = setTimeout(function () { if (call && call.phase === 'ringing') finish('Пропущенный звонок'); }, 45000);
                break;
            case 'accept': if (mine && !call.incoming && call.phase === 'calling') { ringStop(); clearTimeout(call.ringTimeout); call.phase = 'connecting'; render(); startMedia(true); } break;
            case 'offer': if (mine && call.incoming && s.sdp) onOffer(s.sdp); break;
            case 'answer': if (mine && !call.incoming && s.sdp && call.pc) call.pc.setRemoteDescription({ type: 'answer', sdp: s.sdp }).then(remoteReady).catch(function () {}); break;
            case 'ice': if (mine && s.cand) onIce(s.cand); break;
            case 'reject': if (mine) finish('Звонок отклонён'); break;
            case 'busy': if (mine) finish('Абонент занят'); break;
            case 'hangup': if (mine) finish(call.phase === 'ringing' ? 'Пропущенный звонок' : 'Собеседник завершил звонок'); break;
        }
    }

    function poll() {
        fetch('/api/mobile/call/events?after=' + (after == null ? 0 : after), { cache: 'no-store' })
            .then(function (r) { return r.json(); })
            .then(function (j) {
                if (after == null) { after = j.last || 0; return; } // при загрузке страницы старые сигналы не проигрываем
                (j.events || []).forEach(onEvent);
                after = Math.max(after, j.last || 0);
            })
            .catch(function () {})
            .then(function () { setTimeout(poll, 1000); });
    }
    poll();

    window.phoneCall = { start: start };
})();
