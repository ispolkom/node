// ========== Voice Call System ==========
// WebRTC-based P2P voice calling via YANDI signaling

class VoiceCallManager {
    constructor() {
        this.ws = null;
        this.localStream = null;
        this.peerConnection = null;
        this.currentCallId = null;
        this.currentPeerId = null;   // short_id (hex8) of the other party
        this.callModal = null;
        this.incomingModal = null;
        this.isCallActive = false;
        this.isCaller = false;        // true=we initiated, false=we answered
        this.pollingTimer = null;
        this.callingAudio = null;     // incoming ringtone loop
        this.callOutAudio = null;     // outgoing ringtone loop

        this.configuration = {
            iceServers: [
                { urls: 'stun:stun.l.google.com:19302' },
                { urls: 'stun:stun1.l.google.com:19302' },
            ]
        };
    }

    // ── Sound helpers ────────────────────────────────────────────────────────

    _soundEnabled() {
        return window.isSoundEnabled ? window.isSoundEnabled() : true;
    }

    _playRingtone(type) {
        this._stopRingtones();
        if (!this._soundEnabled()) return;
        const src = type === 'calling' ? '/media/calling.mp3' : '/media/call_out.mp3';
        const audio = new Audio(src);
        audio.loop = true;
        audio.volume = 0.7;
        audio.play().catch(() => {});
        if (type === 'calling') this.callingAudio = audio;
        else this.callOutAudio = audio;
    }

    _stopRingtones() {
        if (this.callingAudio) { this.callingAudio.pause(); this.callingAudio.src = ''; this.callingAudio = null; }
        if (this.callOutAudio) { this.callOutAudio.pause(); this.callOutAudio.src = ''; this.callOutAudio = null; }
    }

    // ── Outgoing call ────────────────────────────────────────────────────────

    async startCall(peerId) {
        if (this.isCallActive) {
            this.showNotification('⚠️ Уже есть активный звонок', 'warning');
            return;
        }

        try {
            console.log('📞 Starting call to:', peerId);
            this.isCaller = true;
            this.currentPeerId = peerId;

            // 1. Get mic access
            this.localStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });

            // 2. Open signaling WebSocket
            await this.openSignalingWS(peerId);

            // 3. Tell server to ring the callee
            const resp = await fetch('/api/media/call/start', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    peer_id: peerId,
                    audio_enabled: true,
                    video_enabled: false,
                    display_name: window.myDisplayName || 'Unknown',
                })
            });
            const data = await resp.json();
            if (!resp.ok) throw new Error(data.message || 'Failed to start call');
            this.currentCallId = data.call_id;

            this.showCallModal(`📞 Вызов ${peerId}…`, 'connecting');
            this._playRingtone('call_out');
            console.log('✅ Call started, ID:', this.currentCallId, '— waiting for accept');

        } catch (error) {
            console.error('Failed to start call:', error);
            this.showNotification(`❌ ${error.message}`, 'error');
            this.endCall();
        }
    }

    // ── Incoming call ────────────────────────────────────────────────────────

    startPolling() {
        if (this.pollingTimer) return;
        this.pollingTimer = setInterval(() => this.checkIncomingCalls(), 3000);
    }

    stopPolling() {
        if (this.pollingTimer) {
            clearInterval(this.pollingTimer);
            this.pollingTimer = null;
        }
    }

    async checkIncomingCalls() {
        if (this.isCallActive) return;
        try {
            const resp = await fetch('/api/media/incoming-call');
            const data = await resp.json();
            if (data && data.call_id && !this.incomingModal) {
                this.showIncomingCallModal(data);
            }
        } catch (_) {}
    }

    showIncomingCallModal(callInfo) {
        if (this.incomingModal) return;

        // Play ringtone only if sound is enabled (DND = no sound, but modal still shows)
        this._playRingtone('calling');

        this.incomingModal = document.createElement('div');
        this.incomingModal.className = 'call-modal active incoming-call-modal';
        const callerName = escapeHTML ? escapeHTML(callInfo.from_display_name || callInfo.from_short_id) : (callInfo.from_display_name || callInfo.from_short_id);
        this.incomingModal.innerHTML = `
            <div class="call-modal-content">
                <div class="call-header">
                    <h3>📞 Входящий звонок</h3>
                </div>
                <div class="call-body">
                    <div class="call-peer">От: ${callerName}</div>
                    <div class="call-status" style="color:#fbbf24;">🔔 Звонит…</div>
                    <div class="call-actions" style="gap:12px;">
                        <button class="btn btn-success" style="font-size:1.1em;" id="vcm-accept-btn">✅ Принять</button>
                        <button class="btn btn-danger" id="vcm-reject-btn">❌ Отклонить</button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(this.incomingModal);
        // Привязываем обработчики через closure — данные звонка не попадают в HTML
        const callId = callInfo.call_id;
        const fromShortId = callInfo.from_short_id;
        document.getElementById('vcm-accept-btn').addEventListener('click', () => this.acceptCall(callId, fromShortId));
        document.getElementById('vcm-reject-btn').addEventListener('click', () => this.rejectCall(callId));
    }

    dismissIncomingModal() {
        if (this.incomingModal) {
            this.incomingModal.remove();
            this.incomingModal = null;
        }
    }

    async acceptCall(callId, fromShortId) {
        this._stopRingtones();
        this.dismissIncomingModal();
        try {
            this.isCaller = false;
            this.currentPeerId = fromShortId;
            this.currentCallId = callId;

            // 1. Get mic
            this.localStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });

            // 2. Open signaling WebSocket to caller
            await this.openSignalingWS(fromShortId);

            // 3. Tell server we accepted (sends VoiceCallAccept P2P to caller)
            const resp = await fetch(`/api/media/call/${callId}/accept`, { method: 'POST' });
            const data = await resp.json();
            if (data.status !== 'ok') throw new Error(data.message || 'Accept failed');

            // 4. Create peer connection — wait for offer from caller via WS
            this.createPeerConnection();
            this.isCallActive = true;
            this.showCallModal(`📞 Ответили — ожидаем соединения…`, 'connecting');
            console.log('✅ Call accepted, waiting for WebRTC offer from caller');

        } catch (error) {
            console.error('Failed to accept call:', error);
            this.showNotification(`❌ ${error.message}`, 'error');
            this.endCall();
        }
    }

    async rejectCall(callId) {
        this._stopRingtones();
        this.dismissIncomingModal();
        try {
            await fetch(`/api/media/call/${callId}/reject`, { method: 'POST' });
        } catch (_) {}
    }

    // ── WebSocket signaling ──────────────────────────────────────────────────

    async openSignalingWS(peerId) {
        return new Promise((resolve, reject) => {
            const url = `ws://${window.location.host}/api/media/ws/${peerId}`;
            this.ws = new WebSocket(url);
            this.ws.onopen = () => {
                console.log('🔊 Signaling WS connected to:', peerId);
                resolve();
            };
            this.ws.onerror = (e) => {
                console.error('WS error:', e);
                reject(new Error('WebSocket connection failed'));
            };
            this.ws.onmessage = (e) => this.handleSignaling(e);
            this.ws.onclose = () => {
                console.log('WS closed');
                if (this.isCallActive) this.endCall();
            };
        });
    }

    async handleSignaling(event) {
        let data;
        try {
            data = JSON.parse(event.data);
        } catch (_) { return; }

        console.log('📨 Signaling:', data.type);

        switch (data.type) {
            case 'call-accept':
                // Caller received: callee accepted → stop outgoing ringtone, create offer and send
                if (this.isCaller) {
                    this._stopRingtones();
                    this.isCallActive = true;
                    this.showCallModal('📞 Принято — соединяем…', 'connecting');
                    this.createPeerConnection();
                    await this.createAndSendOffer();
                }
                break;

            case 'call-reject':
                this._stopRingtones();
                this.showNotification('📵 Вызов отклонён', 'warning');
                this.endCall();
                break;

            case 'hangup':
                this._stopRingtones();
                this.showNotification('📞 Звонок завершён', 'info');
                this.endCall();
                break;

            case 'offer':
                await this.handleOffer(data);
                break;

            case 'answer':
                await this.handleAnswer(data);
                break;

            case 'ice-candidate':
                await this.handleIceCandidate(data);
                break;
        }
    }

    // ── WebRTC ───────────────────────────────────────────────────────────────

    createPeerConnection() {
        if (this.peerConnection) return;

        this.peerConnection = new RTCPeerConnection(this.configuration);

        this.localStream.getTracks().forEach(track => {
            this.peerConnection.addTrack(track, this.localStream);
        });

        this.peerConnection.ontrack = (event) => {
            console.log('📡 Received remote audio track');
            this.playRemoteAudio(event.streams[0]);
        };

        this.peerConnection.onicecandidate = (event) => {
            if (event.candidate) this.sendSignal({ type: 'ice-candidate', candidate: event.candidate });
        };

        this.peerConnection.onconnectionstatechange = () => {
            const state = this.peerConnection.connectionState;
            console.log('Connection state:', state);
            if (state === 'connected') {
                this._stopRingtones();
                this.updateCallModal('active');
                this.showNotification('🔊 Соединение установлено', 'success');
            } else if (state === 'disconnected' || state === 'failed') {
                this.endCall();
            }
        };
    }

    async createAndSendOffer() {
        const offer = await this.peerConnection.createOffer();
        await this.peerConnection.setLocalDescription(offer);
        this.sendSignal({ type: 'offer', sdp: offer });
    }

    async handleOffer(offer) {
        if (!this.peerConnection) this.createPeerConnection();
        await this.peerConnection.setRemoteDescription(new RTCSessionDescription(offer.sdp || offer));
        const answer = await this.peerConnection.createAnswer();
        await this.peerConnection.setLocalDescription(answer);
        this.sendSignal({ type: 'answer', sdp: answer });
    }

    async handleAnswer(answer) {
        await this.peerConnection.setRemoteDescription(new RTCSessionDescription(answer.sdp || answer));
    }

    async handleIceCandidate(msg) {
        try {
            await this.peerConnection.addIceCandidate(new RTCIceCandidate(msg.candidate));
        } catch (e) {
            console.error('ICE candidate error:', e);
        }
    }

    sendSignal(msg) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(msg));
        }
    }

    playRemoteAudio(stream) {
        let audio = document.getElementById('yandi-remote-audio');
        if (!audio) {
            audio = document.createElement('audio');
            audio.id = 'yandi-remote-audio';
            audio.autoplay = true;
            document.body.appendChild(audio);
        }
        audio.srcObject = stream;
        audio.play().catch(e => console.warn('Audio play:', e));
    }

    // ── Call lifecycle ───────────────────────────────────────────────────────

    endCall() {
        console.log('📞 Ending call');
        this._stopRingtones();

        // Notify other party
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            try { this.ws.send(JSON.stringify({ type: 'hangup', call_id: this.currentCallId })); } catch (_) {}
        }

        // Also tell server (send VoiceCallEnd P2P)
        if (this.currentPeerId && this.currentCallId) {
            fetch('/api/media/call/end', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ peer_id: this.currentPeerId, call_id: this.currentCallId }),
            }).catch(() => {});
        }

        // Cleanup WebRTC
        if (this.peerConnection) {
            this.peerConnection.close();
            this.peerConnection = null;
        }
        if (this.localStream) {
            this.localStream.getTracks().forEach(t => t.stop());
            this.localStream = null;
        }
        if (this.ws) {
            this.ws.onclose = null;
            this.ws.close();
            this.ws = null;
        }

        // Remove remote audio element
        const audio = document.getElementById('yandi-remote-audio');
        if (audio) audio.remove();

        // Remove modals
        if (this.callModal) { this.callModal.remove(); this.callModal = null; }
        this.dismissIncomingModal();

        this.isCallActive = false;
        this.isCaller = false;
        this.currentCallId = null;
        this.currentPeerId = null;
    }

    toggleMute() {
        if (!this.localStream) return;
        const track = this.localStream.getAudioTracks()[0];
        if (!track) return;
        track.enabled = !track.enabled;
        const btn = document.querySelector('.call-mute-btn');
        if (btn) btn.textContent = track.enabled ? '🔇 Выкл. микрофон' : '🎤 Вкл. микрофон';
    }

    // ── UI helpers ───────────────────────────────────────────────────────────

    showCallModal(message, status) {
        if (this.callModal) this.callModal.remove();
        this.callModal = document.createElement('div');
        this.callModal.className = 'call-modal active';
        this.callModal.innerHTML = `
            <div class="call-modal-content">
                <div class="call-header">
                    <h3>📞 Звонок</h3>
                    <button class="call-close-btn" onclick="window.voiceCallManager.endCall()">✖</button>
                </div>
                <div class="call-body">
                    <div class="call-peer">${this.currentPeerId || ''}</div>
                    <div class="call-status" id="call-status">${message}</div>
                    <div class="call-actions">
                        <button class="btn btn-danger call-end-btn" onclick="window.voiceCallManager.endCall()">🔴 Завершить</button>
                        <button class="btn btn-secondary call-mute-btn" onclick="window.voiceCallManager.toggleMute()">🔇 Выкл. микрофон</button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(this.callModal);
    }

    updateCallModal(status) {
        const el = document.getElementById('call-status');
        if (el && status === 'active') {
            el.textContent = '🟢 Разговор идёт';
            el.style.color = '#22c55e';
        }
    }

    showNotification(msg, type) {
        const n = document.createElement('div');
        n.className = `notification ${type || ''}`;
        n.textContent = msg;
        n.style.cssText = 'position:fixed;top:16px;right:16px;z-index:9999;padding:10px 18px;border-radius:8px;background:#1e293b;color:#fff;font-size:14px;box-shadow:0 2px 8px rgba(0,0,0,.4);';
        document.body.appendChild(n);
        setTimeout(() => n.remove(), 4000);
    }
}

// Global instance
window.voiceCallManager = new VoiceCallManager();

// Start polling for incoming calls when page loads
document.addEventListener('DOMContentLoaded', () => {
    window.voiceCallManager.startPolling();
});
