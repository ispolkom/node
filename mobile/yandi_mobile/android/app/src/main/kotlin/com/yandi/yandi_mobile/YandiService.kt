package com.yandi.yandi_mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.IBinder
import android.util.Base64
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509TrustManager
import kotlin.concurrent.thread
import kotlin.random.Random

/**
 * Фоновый сервис для получения сообщений от ноды.
 *
 * Без Google/Firebase — только стандартная Java/Android:
 *   - TLS cert pinning через javax.net.ssl
 *   - WebSocket RFC 6455 — без OkHttp, реализован вручную
 *   - Адаптивный ping: 30s экран вкл / 90s экран выкл
 *   - Мгновенный reconnect при появлении сети (ConnectivityManager.NetworkCallback)
 *   - Уведомления через стандартный NotificationManager
 */
class YandiChatService : Service() {

    companion object {
        const val ACTION_START   = "com.yandi.chat.START"
        const val ACTION_STOP    = "com.yandi.chat.STOP"
        const val CHANNEL_STATUS = "yandi_status"
        const val CHANNEL_MSG    = "yandi_messages"
        private const val TAG              = "YandiChat"
        private const val NOTIF_ID_STATUS  = 2

        // Интервалы ping: экран вкл = 30s, выкл = 90s
        private const val PING_INTERVAL_SCREEN_ON  = 30_000L
        private const val PING_INTERVAL_SCREEN_OFF = 90_000L

        @Volatile var running = false
    }

    @Volatile private var host        = ""
    @Volatile private var port        = 8766
    @Volatile private var token       = ""
    @Volatile private var fingerprint = ""

    private val screenOn      = AtomicBoolean(true)
    private val lastPongMs    = AtomicLong(System.currentTimeMillis())
    private val networkReady  = AtomicBoolean(true)

    @Volatile private var ws: RawWsClient? = null
    private var workerThread: Thread?      = null
    private var pingThread:   Thread?      = null

    // ── BroadcastReceiver: экран вкл/выкл ─────────────────────────────────────

    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            when (intent?.action) {
                Intent.ACTION_SCREEN_ON  -> screenOn.set(true)
                Intent.ACTION_SCREEN_OFF -> screenOn.set(false)
            }
        }
    }

    // ── ConnectivityManager.NetworkCallback: мгновенный reconnect ─────────────

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (!networkReady.getAndSet(true)) {
                Log.i(TAG, "Сеть восстановлена — переподключение")
                ws?.close()   // прерываем текущее соединение → workerThread переподключится
            }
        }
        override fun onLost(network: Network) {
            networkReady.set(false)
            Log.i(TAG, "Сеть потеряна")
        }
    }

    // ── Android lifecycle ──────────────────────────────────────────────────────

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        // Регистрируем слушатель экрана (только динамически — манифест не работает)
        registerReceiver(screenReceiver, IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_SCREEN_OFF)
        })
        // Регистрируем слушатель сети
        val cm  = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
        val req = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        cm.registerNetworkCallback(req, networkCallback)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                host        = intent.getStringExtra("host")        ?: return START_NOT_STICKY
                port        = intent.getIntExtra("port", 8766)
                token       = intent.getStringExtra("token")       ?: return START_NOT_STICKY
                fingerprint = intent.getStringExtra("fingerprint") ?: ""
                start()
            }
            ACTION_STOP -> stop()
        }
        return START_STICKY
    }

    override fun onDestroy() {
        stop()
        runCatching { unregisterReceiver(screenReceiver) }
        val cm = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
        runCatching { cm.unregisterNetworkCallback(networkCallback) }
        super.onDestroy()
    }

    // ── Start / Stop ───────────────────────────────────────────────────────────

    private fun start() {
        if (running) return
        running = true
        createChannels()
        startForeground(NOTIF_ID_STATUS, buildStatusNotif("Подключение…"))
        startPingThread()
        startWorkerThread()
    }

    private fun stop() {
        running = false
        ws?.close()
        ws = null
        pingThread?.interrupt()
        workerThread?.interrupt()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    // ── Worker: reconnect-loop ─────────────────────────────────────────────────

    private fun startWorkerThread() {
        workerThread = thread(name = "yandi-ws", isDaemon = true) {
            var delay = 5_000L
            while (running) {
                try {
                    updateStatusNotif("Подключено к ноде")
                    val client = RawWsClient(host, port, fingerprint)
                    ws = client
                    lastPongMs.set(System.currentTimeMillis())
                    client.connect("/mobile/ws?token=$token")
                    delay = 5_000L
                    readLoop(client)
                } catch (e: InterruptedException) {
                    break
                } catch (e: Exception) {
                    if (running) Log.w(TAG, "WS ошибка: $e")
                }
                ws = null
                if (!running) break
                updateStatusNotif("Переподключение через ${delay / 1000}с…")
                try { Thread.sleep(delay) } catch (_: InterruptedException) { break }
                delay = (delay * 2).coerceAtMost(60_000L)
            }
        }
    }

    // ── Ping thread: адаптивный интервал ──────────────────────────────────────

    private fun startPingThread() {
        pingThread = thread(name = "yandi-ping", isDaemon = true) {
            while (running) {
                val interval = if (screenOn.get()) PING_INTERVAL_SCREEN_ON
                               else                PING_INTERVAL_SCREEN_OFF
                try {
                    Thread.sleep(interval)
                } catch (_: InterruptedException) {
                    break
                }
                val client = ws ?: continue
                val pongAge = System.currentTimeMillis() - lastPongMs.get()
                // Если нет ответа дольше 2 интервалов — принудительно переподключаемся
                if (pongAge > interval * 2) {
                    Log.w(TAG, "Pong timeout (${pongAge}ms) — переподключение")
                    client.close()
                    continue
                }
                try {
                    // Посылаем наш FT_PING (0x01) — нода ответит FT_PONG (0x02)
                    client.sendFrame(0x02, byteArrayOf(0x01))
                } catch (_: Exception) { /* readLoop обнаружит разрыв */ }
            }
        }
    }

    // ── WebSocket read loop ────────────────────────────────────────────────────

    private fun readLoop(client: RawWsClient) {
        while (running) {
            val (opcode, payload) = client.readFrame() ?: break
            when (opcode) {
                0x08 -> break                            // Close
                0x09 -> client.sendFrame(0x0A, payload) // WebSocket Ping → Pong
                0x02 -> handleBinaryFrame(payload)       // Бинарный фрейм YANDI-протокола
            }
        }
    }

    private fun handleBinaryFrame(data: ByteArray) {
        if (data.isEmpty()) return
        when (data[0]) {
            // FT_PING от ноды → отвечаем FT_PONG
            0x01.toByte() -> {
                lastPongMs.set(System.currentTimeMillis())
                ws?.sendFrame(0x02, byteArrayOf(0x02))
            }

            // FT_PONG — нода подтвердила наш ping
            0x02.toByte() -> lastPongMs.set(System.currentTimeMillis())

            // FT_CHAT_MSG: [1B type][32B from][8B ts_ms_LE][4B text_len_LE][text]
            0x10.toByte() -> {
                if (data.size < 45) return
                val from    = data.slice(1..32).toByteArray().toHex()
                val textLen = data.slice(41..44).toByteArray().toInt32LE()
                if (data.size < 45 + textLen) return
                val text    = String(data, 45, textLen, Charsets.UTF_8)
                showMessageNotif(from.take(8) + "…", text)
            }
        }
    }

    // ── Notifications ──────────────────────────────────────────────────────────

    private fun createChannels() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_STATUS, "YANDI статус",
                NotificationManager.IMPORTANCE_LOW).apply {
                description = "Статус подключения к ноде"
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_MSG, "YANDI сообщения",
                NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Входящие сообщения"
            }
        )
    }

    private fun buildStatusNotif(text: String): Notification {
        val intent = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL_STATUS)
            .setSmallIcon(android.R.drawable.ic_menu_send)
            .setContentTitle("YANDI")
            .setContentText(text)
            .setOngoing(true)
            .setContentIntent(intent)
            .build()
    }

    private fun updateStatusNotif(text: String) {
        getSystemService(NotificationManager::class.java)
            .notify(NOTIF_ID_STATUS, buildStatusNotif(text))
    }

    private fun showMessageNotif(fromShort: String, text: String) {
        val intent = PendingIntent.getActivity(
            this, System.currentTimeMillis().toInt(),
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notif = NotificationCompat.Builder(this, CHANNEL_MSG)
            .setSmallIcon(android.R.drawable.ic_dialog_email)
            .setContentTitle("Сообщение от $fromShort")
            .setContentText(text.take(200))
            .setAutoCancel(true)
            .setContentIntent(intent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        getSystemService(NotificationManager::class.java)
            .notify(System.currentTimeMillis().toInt(), notif)
    }

    // ── Helpers ────────────────────────────────────────────────────────────────

    private fun ByteArray.toHex() = joinToString("") { "%02x".format(it) }

    private fun ByteArray.toInt32LE(): Int {
        var v = 0
        for (i in 0..3) v = v or ((this[i].toInt() and 0xFF) shl (i * 8))
        return v
    }
}

// ── Minimal WebSocket client (RFC 6455, без внешних зависимостей) ──────────────

/**
 * Минимальный WebSocket-клиент поверх TLS.
 * TLS: javax.net.ssl + кастомный TrustManager для cert pinning по SHA-256.
 * WS: HTTP Upgrade + бинарные фреймы RFC 6455.
 */
class RawWsClient(
    private val host:        String,
    private val port:        Int,
    private val fingerprint: String,
) {
    private lateinit var ssl:    SSLSocket
    private lateinit var input:  InputStream
    private lateinit var output: OutputStream
    private val writeLock = Any()

    fun connect(path: String) {
        ssl    = buildTlsSocket()
        input  = ssl.inputStream
        output = ssl.outputStream

        val key = Base64.encodeToString(Random.nextBytes(16), Base64.NO_WRAP)
        val req = buildString {
            append("GET $path HTTP/1.1\r\n")
            append("Host: $host:$port\r\n")
            append("Upgrade: websocket\r\n")
            append("Connection: Upgrade\r\n")
            append("Sec-WebSocket-Key: $key\r\n")
            append("Sec-WebSocket-Version: 13\r\n")
            append("\r\n")
        }
        output.write(req.toByteArray(Charsets.US_ASCII))

        val response = readHttpHeaders()
        if (!response.contains("101")) throw Exception("WS upgrade failed: $response")
    }

    fun readFrame(): Pair<Int, ByteArray>? {
        val b0 = input.read(); if (b0 < 0) return null
        val b1 = input.read(); if (b1 < 0) return null

        val opcode = b0 and 0x0F
        val masked  = (b1 and 0x80) != 0

        var payloadLen = (b1 and 0x7F).toLong()
        payloadLen = when (payloadLen.toInt()) {
            126  -> ((input.read() shl 8) or input.read()).toLong()
            127  -> { var v = 0L; repeat(8) { v = (v shl 8) or input.read().toLong() }; v }
            else -> payloadLen
        }

        val mask    = if (masked) ByteArray(4).also { readFully(it) } else null
        val payload = ByteArray(payloadLen.toInt()).also { readFully(it) }
        if (mask != null) {
            for (i in payload.indices) payload[i] = (payload[i].toInt() xor mask[i % 4].toInt()).toByte()
        }
        return Pair(opcode, payload)
    }

    fun sendFrame(opcode: Int, data: ByteArray) {
        val mask   = Random.nextBytes(4)
        val masked = ByteArray(data.size) { i -> (data[i].toInt() xor mask[i % 4].toInt()).toByte() }

        val buf = ByteArrayOutputStream()
        buf.write(0x80 or opcode)
        when {
            data.size < 126   -> buf.write(0x80 or data.size)
            data.size < 65536 -> {
                buf.write(0x80 or 126)
                buf.write(data.size ushr 8); buf.write(data.size and 0xFF)
            }
            else -> {
                buf.write(0x80 or 127)
                val len = data.size.toLong()
                for (i in 7 downTo 0) buf.write((len ushr (i * 8)).toInt() and 0xFF)
            }
        }
        buf.write(mask)
        buf.write(masked)
        synchronized(writeLock) { output.write(buf.toByteArray()) }
    }

    fun close() {
        runCatching { sendFrame(0x08, byteArrayOf()) }
        runCatching { ssl.close() }
    }

    private fun buildTlsSocket(): SSLSocket {
        val tm = object : X509TrustManager {
            override fun checkClientTrusted(chain: Array<X509Certificate>, t: String) {}
            override fun getAcceptedIssuers(): Array<X509Certificate> = arrayOf()
            override fun checkServerTrusted(chain: Array<X509Certificate>, t: String) {
                if (fingerprint.isEmpty()) return
                val fp = MessageDigest.getInstance("SHA-256")
                    .digest(chain[0].encoded)
                    .joinToString("") { "%02x".format(it) }
                if (fp != fingerprint.lowercase())
                    throw CertificateException("TLS fingerprint mismatch: $fp != $fingerprint")
            }
        }
        val ctx = SSLContext.getInstance("TLS")
        ctx.init(null, arrayOf(tm), null)
        val sock = ctx.socketFactory.createSocket() as SSLSocket
        sock.connect(InetSocketAddress(host, port), 10_000)
        sock.soTimeout = 120_000   // 2 мин — достаточно для 90s ping интервала
        sock.startHandshake()
        return sock
    }

    private fun readFully(buf: ByteArray) {
        var off = 0
        while (off < buf.size) {
            val n = input.read(buf, off, buf.size - off)
            if (n < 0) throw Exception("EOF")
            off += n
        }
    }

    private fun readHttpHeaders(): String {
        val sb = StringBuilder()
        val buf = ByteArray(1)
        while (true) {
            val n = input.read(buf)
            if (n < 0) throw Exception("EOF reading HTTP headers")
            sb.append(buf[0].toInt().toChar())
            if (sb.endsWith("\r\n\r\n")) break
            if (sb.length > 8192) throw Exception("HTTP headers too large")
        }
        return sb.toString()
    }
}
