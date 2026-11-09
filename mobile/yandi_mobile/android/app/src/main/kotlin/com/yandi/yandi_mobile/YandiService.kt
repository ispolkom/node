package com.yandi.yandi_mobile

import android.app.ActivityManager
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
import android.net.Uri
import android.media.AudioAttributes
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Base64
import android.util.Log
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.core.app.NotificationCompat
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.io.File
import java.nio.ByteBuffer
import java.net.InetSocketAddress
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
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
        // New ID: Android keeps the sound of an existing channel unchanged.
        const val CHANNEL_MSG    = "yandi_messages_icq_v1"
        const val CHANNEL_CALLS = "yandi_calls_calling_v1"
        const val CHANNEL_MSG_SILENT = "yandi_messages_silent"
        private const val TAG              = "YandiChat"
        private const val NOTIF_ID_STATUS  = 2

        // Интервалы ping: экран вкл = 30s, выкл = 180s, Doze = 300s
        private const val PING_INTERVAL_SCREEN_ON  = 30_000L
        private const val PING_INTERVAL_SCREEN_OFF = 180_000L
        private const val PING_INTERVAL_DOZE = 300_000L

        @Volatile var running = false

        private const val PENDING_LIVE_FILE = "yandi_pending_live.bin"
        private const val PREFS = "yandi_background_service"
        private const val KEY_ALIAS = "yandi_background_token_v1"

        @Synchronized
        fun drainPendingLive(context: Context): ArrayList<ByteArray> {
            val file = File(context.filesDir, PENDING_LIVE_FILE)
            if (!file.exists()) return arrayListOf()
            return runCatching {
                val bytes = file.readBytes()
                val result = arrayListOf<ByteArray>()
                var offset = 0
                while (offset + 4 <= bytes.size) {
                    val size = ByteBuffer.wrap(bytes, offset, 4).int
                    offset += 4
                    if (size <= 0 || size > 45 + 32 * 1024 || offset + size > bytes.size) break
                    result.add(bytes.copyOfRange(offset, offset + size))
                    offset += size
                }
                file.delete()
                result
            }.getOrDefault(arrayListOf())
        }

        private fun secretKey(): SecretKey {
            val ks = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (ks.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
            return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
                init(KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setUserAuthenticationRequired(false)
                    .build())
            }.generateKey()
        }

        private fun encryptToken(token: String): String {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, secretKey()) }
            val packed = cipher.iv + cipher.doFinal(token.toByteArray(Charsets.UTF_8))
            return Base64.encodeToString(packed, Base64.NO_WRAP)
        }

        private fun decryptToken(value: String): String? = runCatching {
            val packed = Base64.decode(value, Base64.NO_WRAP)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, secretKey(), javax.crypto.spec.GCMParameterSpec(128, packed.copyOfRange(0, 12)))
            cipher.doFinal(packed.copyOfRange(12, packed.size)).toString(Charsets.UTF_8)
        }.getOrNull()

        fun saveConfig(context: Context, host: String, port: Int, token: String, fingerprint: String) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString("host", host).putInt("port", port).putString("token", encryptToken(token)).putString("fingerprint", fingerprint).apply()
        }

        fun loadConfig(context: Context): Array<String>? {
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val host = prefs.getString("host", null) ?: return null
            val encrypted = prefs.getString("token", null) ?: return null
            val token = decryptToken(encrypted) ?: return null
            return arrayOf(host, prefs.getInt("port", 8766).toString(), token, prefs.getString("fingerprint", "") ?: "")
        }
    }

    @Volatile private var host        = ""
    @Volatile private var port        = 8766
    @Volatile private var token       = ""
    @Volatile private var fingerprint = ""

    private val screenOn      = AtomicBoolean(true)
    private val deviceIdle  = AtomicBoolean(false)
    private val lastPongMs    = AtomicLong(System.currentTimeMillis())
    private val networkReady  = AtomicBoolean(true)
    /** Поколение соединения: потоки прежней настройки видят, что номер сменился, и выходят, не трогая новую. */
    private val generation    = java.util.concurrent.atomic.AtomicInteger(0)

    @Volatile private var ws: RawWsClient? = null
    private var workerThread: Thread?      = null
    private var pingThread:   Thread?      = null

    // ── BroadcastReceiver: экран вкл/выкл ─────────────────────────────────────

    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            when (intent?.action) {
                Intent.ACTION_SCREEN_ON  -> screenOn.set(true)
                Intent.ACTION_SCREEN_OFF -> screenOn.set(false)
                PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED -> {
                    val pm = getSystemService(POWER_SERVICE) as? PowerManager
                    deviceIdle.set(pm?.isDeviceIdleMode == true)
                }
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
        val pm = getSystemService(POWER_SERVICE) as? PowerManager
        screenOn.set(pm?.isInteractive != false)
        deviceIdle.set(pm?.isDeviceIdleMode == true)
        registerReceiver(screenReceiver, IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_SCREEN_OFF)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                addAction(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED)
            }
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
                val stored = loadConfig(this)
                val newHost  = intent.getStringExtra("host") ?: stored?.getOrNull(0) ?: return START_NOT_STICKY
                val newPort  = if (intent.hasExtra("port")) intent.getIntExtra("port", 8766) else stored?.getOrNull(1)?.toIntOrNull() ?: 8766
                val newToken = intent.getStringExtra("token") ?: stored?.getOrNull(2) ?: return START_NOT_STICKY
                val newPrint = intent.getStringExtra("fingerprint") ?: stored?.getOrNull(3) ?: ""
                // смена узла (или токена) у работающей службы: соединение прежнего узла закрывается ДО нового запуска,
                // иначе оно жило бы со старым токеном и продолжало получать уведомления удалённого узла
                val changed = running && (newHost != host || newPort != port || newToken != token || newPrint != fingerprint)
                if (changed) {
                    generation.incrementAndGet()
                    running = false
                    ws?.close()
                    ws = null
                    pingThread?.interrupt()
                    workerThread?.interrupt()
                }
                host = newHost; port = newPort; token = newToken; fingerprint = newPrint
                saveConfig(this, newHost, newPort, newToken, newPrint)
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
        generation.incrementAndGet()
        createChannels()
        startForeground(NOTIF_ID_STATUS, buildStatusNotif("Подключение…"))
        startPingThread()
        startWorkerThread()
    }

    private fun stop() {
        running = false
        generation.incrementAndGet()
        ws?.close()
        ws = null
        pingThread?.interrupt()
        workerThread?.interrupt()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    // ── Worker: reconnect-loop ─────────────────────────────────────────────────

    private fun startWorkerThread() {
        val gen = generation.get()
        workerThread = thread(name = "yandi-ws", isDaemon = true) {
            var delay = 5_000L
            while (running && gen == generation.get()) {
                var client: RawWsClient? = null
                try {
                    updateStatusNotif("Подключено к ноде")
                    val c = RawWsClient(host, port, fingerprint)
                    client = c
                    ws = c
                    lastPongMs.set(System.currentTimeMillis())
                    c.connect("/mobile/ws", token)
                    if (gen != generation.get()) { c.close(); break }   // за время соединения узел сменили: это уже чужое поколение
                    delay = 5_000L
                    readLoop(c)
                } catch (e: InterruptedException) {
                    break
                } catch (e: Exception) {
                    if (running) Log.w(TAG, "WS ошибка: ${e.javaClass.simpleName}")
                } finally {
                    // сокет закрывается на любом выходе (ошибка рукопожатия, кривой кадр, таймаут), иначе дескрипторы копятся до GC
                    runCatching { client?.close() }
                }
                if (gen == generation.get()) ws = null
                if (!running || gen != generation.get()) break
                updateStatusNotif("Переподключение через ${delay / 1000}с…")
                try { Thread.sleep(delay) } catch (_: InterruptedException) { break }
                delay = (delay * 2).coerceAtMost(60_000L)
            }
        }
    }

    // ── Ping thread: адаптивный интервал ──────────────────────────────────────

    private fun startPingThread() {
        val gen = generation.get()
        pingThread = thread(name = "yandi-ping", isDaemon = true) {
            while (running && gen == generation.get()) {
                val interval = when {
                    screenOn.get() -> PING_INTERVAL_SCREEN_ON
                    deviceIdle.get() -> PING_INTERVAL_DOZE
                    else -> PING_INTERVAL_SCREEN_OFF
                }
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
                // Payload is authenticated E4. Never decrypt or parse its
                // signed header here: Dart owns key pins and durable replay state.
                if (data.size < 45) return
                val from    = data.slice(1..32).toByteArray().toHex()
                val textLen = data.slice(41..44).toByteArray().toInt32LE()
                if (data.size < 45 + textLen) return
                // Содержимое зашифровано для приложения (E2E), ключей у фонового сервиса нет: текст не показываем никогда — только
                // «есть новое». Пока приложение открыто, уведомления показывает оно само (оно же расшифровывает и отличает квитанции).
                if (appInForeground()) return
                showMessageNotif(from)
            }

            // FT_LIVE_MSG: сигнал звонка зашифрован для Flutter/E2E. Native-слой
            // только сохраняет пакет и будит приложение; он не доверяет его содержимому.
            0x13.toByte() -> {
                if (data.size < 45 || data.size > 45 + 32 * 1024) return
                val payloadLen = data.slice(41..44).toByteArray().toInt32LE()
                if (payloadLen < 1 || data.size < 45 + payloadLen) return
                if (appInForeground()) return
                persistPendingLive(data)
                showCallWakeNotif()
            }
        }
    }

    @Synchronized
    private fun persistPendingLive(data: ByteArray) {
        runCatching {
            val file = File(filesDir, PENDING_LIVE_FILE)
            // Ограничиваем очередь: старые сигналы звонка уже неактуальны.
            val existing = if (file.exists()) file.readBytes() else ByteArray(0)
            val record = ByteBuffer.allocate(4).putInt(data.size).array() + data
            val combined = (existing + record)
            val maxBytes = (45 + 32 * 1024) * 8
            file.writeBytes(if (combined.size > maxBytes) combined.takeLast(maxBytes).toByteArray() else combined)
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
                setSound(Uri.parse("android.resource://$packageName/raw/icq"),
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_NOTIFICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_CALLS, "YANDI звонки",
                NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Входящие аудио- и видеозвонки"
                setSound(Uri.parse("android.resource://$packageName/raw/calling"),
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_MSG_SILENT, "YANDI сообщения без звука",
                NotificationManager.IMPORTANCE_HIGH).apply {
                setSound(null, null)
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

    private fun appInForeground(): Boolean {
        val info = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(info)
        return info.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }

    /** Одно уведомление на собеседника; повтор (например, квитанция, которую сервис не может отличить) — без звука. */
    private fun showMessageNotif(from: String) {
        val intent = PendingIntent.getActivity(
            this, System.currentTimeMillis().toInt(),
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val soundEnabled = getSharedPreferences("yandi_alerts", MODE_PRIVATE)
            .getBoolean("messages", true)
        val notif = NotificationCompat.Builder(this, if (soundEnabled) CHANNEL_MSG else CHANNEL_MSG_SILENT)
            .setSound(if (soundEnabled) Uri.parse("android.resource://$packageName/raw/icq") else null)
            .setSmallIcon(android.R.drawable.ic_dialog_email)
            .setContentTitle("YANDI")
            .setContentText("🔒 Новое сообщение — откройте YANDI")
            .setOnlyAlertOnce(true)
            .setAutoCancel(true)
            .setContentIntent(intent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        getSystemService(NotificationManager::class.java)
            .notify(from.hashCode(), notif)
    }

    private fun showCallWakeNotif() {
        val intent = PendingIntent.getActivity(
            this, System.currentTimeMillis().toInt(),
            Intent(this, MainActivity::class.java).apply { flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notif = NotificationCompat.Builder(this, CHANNEL_CALLS)
            .setSmallIcon(android.R.drawable.ic_menu_call)
            .setContentTitle("YANDI")
            .setContentText("Входящий звонок")
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setFullScreenIntent(intent, true)
            .setAutoCancel(true)
            .setTimeoutAfter(45_000)
            .setContentIntent(intent)
            .build()
        getSystemService(NotificationManager::class.java).notify(0x0CA11, notif)
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
/** Наибольший допустимый кадр WS от узла (текст, сигналы, служебное); файлы идут по HTTP кусками. */
private const val MAX_WS_FRAME = 4L * 1024 * 1024

class RawWsClient(
    private val host:        String,
    private val port:        Int,
    private val fingerprint: String,
) {
    private lateinit var ssl:    SSLSocket
    private lateinit var input:  InputStream
    private lateinit var output: OutputStream
    private val writeLock = Any()

    /** Токен устройства — только в заголовке Authorization (#4 аудита): узел больше не принимает его в адресе. */
    fun connect(path: String, token: String) {
        ssl    = buildTlsSocket()
        input  = ssl.inputStream
        output = ssl.outputStream

        val key = Base64.encodeToString(Random.nextBytes(16), Base64.NO_WRAP)
        val req = buildString {
            append("GET $path HTTP/1.1\r\n")
            append("Host: $host:$port\r\n")
            append("Upgrade: websocket\r\n")
            append("Connection: Upgrade\r\n")
            append("Authorization: Bearer $token\r\n")
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

        // длина кадра приходит от узла: выделять память под неё можно только в разумных пределах (кадры YANDI — текст и сигналы, файлы идут по HTTP)
        if (payloadLen < 0 || payloadLen > MAX_WS_FRAME) throw java.io.IOException("WS frame too large: $payloadLen")
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

    /** Закрывает транспорт безусловно: вежливый Close-кадр не отправляется, чтобы не ждать writeLock зависшей записи. */
    fun close() {
        if (::ssl.isInitialized) runCatching { ssl.close() }
    }

    private fun buildTlsSocket(): SSLSocket {
        val tm = PinnedTrustManager(fingerprint)
        val ctx = SSLContext.getInstance("TLS")
        ctx.init(null, arrayOf(tm), null)
        val sock = ctx.socketFactory.createSocket() as SSLSocket
        try {
            sock.connect(InetSocketAddress(host, port), 10_000)
            sock.soTimeout = 360_000   // запас для 180–300s low-power ping интервала
            sock.keepAlive = true
            sock.startHandshake()
        } catch (e: Exception) {
            runCatching { sock.close() }
            throw e
        }
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
