// ========== Video Call System ==========
// WebRTC-based P2P video calling via YANDI signaling
// Архитектура идентична voice-call.js: offer отправляется только после call-accept

class VideoCallManager {
    constructor() {
        this.ws = null;
        this.localStream = null;
        this.peerConnection = null;
        this.currentCallId = null;
        this.currentPeerId = null;
        this.callModal = null;
        this.incomingModal = null;
        this.isCallActive = false;
        this.isCaller = false;
        this.pollingTimer = null;
        this.callingAudio = null;
        this.callOutAudio = null;

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

    async startVideoCall(peerId) {
        if (this.isCallActive) {
            this.showNotification('⚠️ Уже есть активный звонок', 'warning');
            return;
        }
        if (window.voiceCallManager && window.voiceCallManager.isCallActive) {
            this.showNotification('⚠️ Сначала завершите голосовой звонок', 'warning');
            return;
        }

        try {
            console.log('🎥 Starting video call to:', peerId);
            this.isCaller = true;
            this.currentPeerId = peerId;

            this.localStream = await navigator.mediaDevices.getUserMedia({
                audio: true,
                video: { width: { ideal: 1280 }, height: { ideal: 720 }, frameRate: { ideal: 30 } }
            });

            await this.openSignalingWS(peerId);

            const resp = await fetch('/api/media/video/call/start', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    peer_id: peerId,
                    audio_enabled: true,
                    video_enabled: true,
                    display_name: window.myDisplayName || 'Unknown',
                })
            });
            const data = await resp.json();
            if (!resp.ok) throw new Error(data.message || 'Failed to start video call');
            this.currentCallId = data.call_id;

            this.showVideoModal(`🎥 Видеовызов ${peerId}…`, 'connecting');
            this._playRingtone('call_out');
            console.log('✅ Video call started, ID:', this.currentCallId);

        } catch (error) {
            console.error('Failed to start video call:', error);
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
            const resp = await fetch('/api/media/video/incoming-call');
            const data = await resp.json();
            if (data && data.call_id && !this.incomingModal) {
                this.showIncomingCallModal(data);
            }
        } catch (_) {}
    }

    showIncomingCallModal(callInfo) {
        if (this.incomingModal) return;

        this._playRingtone('calling');

        this.incomingModal = document.createElement('div');
        this.incomingModal.className = 'call-modal active incoming-call-modal';
        const callerName = escapeHTML ? escapeHTML(callInfo.from_display_name || callInfo.from_short_id) : (callInfo.from_display_name || callInfo.from_short_id);
        this.incomingModal.innerHTML = `
            <div class="call-modal-content">
                <div class="call-header">
                    <h3>🎥 Входящий видеозвонок</h3>
                </div>
                <div class="call-body">
                    <div class="call-peer">От: ${callerName}</div>
                    <div class="call-status" style="color:#fbbf24;">🔔 Видеовызов…</div>
                    <div class="call-actions" style="gap:12px;">
                        <button class="btn btn-success" style="font-size:1.1em;" id="vcm-video-accept-btn">🎥 Принять</button>
                        <button class="btn btn-danger" id="vcm-video-reject-btn">❌ Отклонить</button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(this.incomingModal);
        const callId = callInfo.call_id;
        const fromShortId = callInfo.from_short_id;
        document.getElementById('vcm-video-accept-btn').addEventListener('click', () => this.acceptCall(callId, fromShortId));
        document.getElementById('vcm-video-reject-btn').addEventListener('click', () => this.rejectCall(callId));
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

            this.localStream = await navigator.mediaDevices.getUserMedia({
                audio: true,
                video: { width: { ideal: 1280 }, height: { ideal: 720 } }
            });

            await this.openSignalingWS(fromShortId);

            const resp = await fetch(`/api/media/video/call/${callId}/accept`, { method: 'POST' });
            const data = await resp.json();
            if (data.status !== 'ok') throw new Error(data.message || 'Accept failed');

            this.createPeerConnection();
            this.isCallActive = true;
            this.showVideoModal('🎥 Ответили — ожидаем соединения…', 'connecting');
            console.log('✅ Video call accepted, waiting for WebRTC offer from caller');

        } catch (error) {
            console.error('Failed to accept video call:', error);
            this.showNotification(`❌ ${error.message}`, 'error');
            this.endCall();
        }
    }

    async rejectCall(callId) {
        this._stopRingtones();
        this.dismissIncomingModal();
        try {
            await fetch(`/api/media/video/call/${callId}/reject`, { method: 'POST' });
        } catch (_) {}
    }

    // ── WebSocket signaling ──────────────────────────────────────────────────

    async openSignalingWS(peerId) {
        return new Promise((resolve, reject) => {
            const url = `ws://${window.location.host}/api/media/ws/${peerId}`;
            this.ws = new WebSocket(url);
            this.ws.onopen = () => { resolve(); };
            this.ws.onerror = () => { reject(new Error('WebSocket connection failed')); };
            this.ws.onmessage = (e) => this.handleSignaling(e);
            this.ws.onclose = () => {
                if (this.isCallActive) this.endCall();
            };
        });
    }

    async handleSignaling(event) {
        let data;
        try { data = JSON.parse(event.data); } catch (_) { return; }

        // Ignore voice-call signals (no call_type or call_type=voice)
        if (data.call_type && data.call_type !== 'video' && data.type !== 'offer' && data.type !== 'answer' && data.type !== 'ice-candidate') return;

        console.log('📨 Video signaling:', data.type);

        switch (data.type) {
            case 'call-accept':
                if (data.call_type !== 'video') return;
                if (this.isCaller) {
                    this._stopRingtones();
                    this.isCallActive = true;
                    this.showVideoModal('🎥 Принято — соединяем…', 'connecting');
                    this.createPeerConnection();
                    await this.createAndSendOffer();
                }
                break;

            case 'call-reject':
                if (data.call_type && data.call_type !== 'video') return;
                this._stopRingtones();
                this.showNotification('📵 Видеовызов отклонён', 'warning');
                this.endCall();
                break;

            case 'hangup':
                if (data.call_type && data.call_type !== 'video') return;
                this._stopRingtones();
                this.showNotification('🎥 Видеозвонок завершён', 'info');
                this.endCall();
                break;

            case 'video-offer':
                await this.handleOffer(data);
                break;

            case 'video-answer':
                await this.handleAnswer(data);
                break;

            case 'video-ice':
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
            console.log('📡 Received remote track:', event.track.kind);
            this.displayRemoteStream(event.streams[0]);
        };

        this.peerConnection.onicecandidate = (event) => {
            if (event.candidate) this.sendSignal({ type: 'video-ice', candidate: event.candidate });
        };

        this.peerConnection.onconnectionstatechange = () => {
            const state = this.peerConnection.connectionState;
            console.log('Video connection state:', state);
            if (state === 'connected') {
                this._stopRingtones();
                this.updateModalStatus('active');
                this.showNotification('🎥 Видеосвязь установлена', 'success');
            } else if (state === 'disconnected' || state === 'failed') {
                this.endCall();
            }
        };

        // Show local video preview
        this.displayLocalStream();
    }

    async createAndSendOffer() {
        const offer = await this.peerConnection.createOffer();
        await this.peerConnection.setLocalDescription(offer);
        this.sendSignal({ type: 'video-offer', sdp: offer });
    }

    async handleOffer(offer) {
        if (!this.peerConnection) this.createPeerConnection();
        await this.peerConnection.setRemoteDescription(new RTCSessionDescription(offer.sdp || offer));
        const answer = await this.peerConnection.createAnswer();
        await this.peerConnection.setLocalDescription(answer);
        this.sendSignal({ type: 'video-answer', sdp: answer });
    }

    async handleAnswer(answer) {
        await this.peerConnection.setRemoteDescription(new RTCSessionDescription(answer.sdp || answer));
    }

    async handleIceCandidate(msg) {
        try {
            await this.peerConnection.addIceCandidate(new RTCIceCandidate(msg.candidate));
        } catch (e) {
            console.error('Video ICE candidate error:', e);
        }
    }

    sendSignal(msg) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(msg));
        }
    }

    displayLocalStream() {
        let video = document.getElementById('yandi-local-video');
        if (!video) return;
        video.srcObject = this.localStream;
        video.play().catch(() => {});
    }

    displayRemoteStream(stream) {
        let video = document.getElementById('yandi-remote-video');
        if (!video) return;
        video.srcObject = stream;
        video.play().catch(() => {});
    }

    // ── Call lifecycle ───────────────────────────────────────────────────────

    endCall() {
        console.log('🎥 Ending video call');
        this._stopRingtones();

        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            try { this.ws.send(JSON.stringify({ type: 'hangup', call_type: 'video', call_id: this.currentCallId })); } catch (_) {}
        }

        if (this.currentPeerId && this.currentCallId) {
            fetch('/api/media/video/call/end', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ peer_id: this.currentPeerId, call_id: this.currentCallId }),
            }).catch(() => {});
        }

        if (this.peerConnection) { this.peerConnection.close(); this.peerConnection = null; }
        if (this.localStream) { this.localStream.getTracks().forEach(t => t.stop()); this.localStream = null; }
        if (this.ws) { this.ws.onclose = null; this.ws.close(); this.ws = null; }

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
        const btn = document.querySelector('.video-mute-btn');
        if (btn) btn.textContent = track.enabled ? '🔇 Выкл. mic' : '🎤 Вкл. mic';
    }

    toggleVideo() {
        if (!this.localStream) return;
        const track = this.localStream.getVideoTracks()[0];
        if (!track) return;
        track.enabled = !track.enabled;
        const btn = document.querySelector('.video-cam-btn');
        if (btn) btn.textContent = track.enabled ? '📷 Выкл. камеру' : '🎥 Вкл. камеру';
    }

    // ── UI helpers ───────────────────────────────────────────────────────────

    showVideoModal(message, status) {
        if (this.callModal) this.callModal.remove();
        this.callModal = document.createElement('div');
        this.callModal.className = 'call-modal active video-call-modal';
        this.callModal.innerHTML = `
            <div class="call-modal-content video-modal-content">
                <div class="call-header">
                    <h3>🎥 Видеозвонок</h3>
                    <button class="call-close-btn" onclick="window.videoCallManager.endCall()">✖</button>
                </div>
                <div class="call-body">
                    <div class="video-container">
                        <video id="yandi-remote-video" autoplay playsinline style="width:100%;max-height:360px;background:#000;border-radius:8px;"></video>
                        <video id="yandi-local-video" autoplay playsinline muted style="position:absolute;bottom:8px;right:8px;width:120px;height:90px;background:#222;border-radius:6px;border:2px solid #334155;object-fit:cover;"></video>
                    </div>
                    <div class="call-peer">${this.currentPeerId || ''}</div>
                    <div class="call-status" id="video-call-status">${message}</div>
                    <div class="call-actions">
                        <button class="btn btn-danger" onclick="window.videoCallManager.endCall()">🔴 Завершить</button>
                        <button class="btn btn-secondary video-mute-btn" onclick="window.videoCallManager.toggleMute()">🔇 Выкл. mic</button>
                        <button class="btn btn-secondary video-cam-btn" onclick="window.videoCallManager.toggleVideo()">📷 Выкл. камеру</button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(this.callModal);

        // Показать локальное видео сразу после создания элемента
        if (this.localStream) {
            setTimeout(() => this.displayLocalStream(), 50);
        }
    }

    updateModalStatus(status) {
        const el = document.getElementById('video-call-status');
        if (el && status === 'active') {
            el.textContent = '🟢 Видеосвязь идёт';
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
window.videoCallManager = new VideoCallManager();

// Start polling for incoming video calls when page loads
document.addEventListener('DOMContentLoaded', () => {
    window.videoCallManager.startPolling();
});
