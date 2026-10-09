package com.yandi.yandi_mobile

import android.content.Intent
import android.net.ProxyInfo
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import java.io.BufferedInputStream
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.atomic.AtomicLong
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager
import kotlin.concurrent.thread

/**
 * Выход телефона в интернет через свой компьютер.
 *
 * Как это устроено: на телефоне поднимается маленький прокси на 127.0.0.1 (понимает HTTP CONNECT и SOCKS5). Каждое соединение он
 * открывает до компьютера по TLS (сертификат сверяется по отпечатку из QR) и просит `CONNECT цель` с токеном устройства. Системный
 * режим VPN нужен только затем, чтобы объявить этот прокси системе: браузеры и приложения, которые уважают системный прокси, идут через
 * него сами. Маршрутов в туннель не добавляется, поэтому остальной трафик (в том числе сам канал до компьютера) идёт напрямую.
 * Приложения, которые системный прокси игнорируют, или UDP/QUIC этим способом не охватываются.
 */
class YandiVpnService : VpnService() {

    companion object {
        const val ACTION_START = "com.yandi.vpn.START"
        const val ACTION_STOP  = "com.yandi.vpn.STOP"
        private const val TAG = "YandiVpn"
        const val LOCAL_PORT = 10808

        @Volatile var running = false
        val bytesUp   = AtomicLong(0)
        val bytesDown = AtomicLong(0)
        val connections = AtomicLong(0)
    }

    private var tun: ParcelFileDescriptor? = null
    private var proxy: LocalProxy? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                val host = intent.getStringExtra("host") ?: return START_NOT_STICKY
                val port = intent.getIntExtra("port", 443)
                val fp   = intent.getStringExtra("fingerprint") ?: ""
                val tok  = intent.getStringExtra("token") ?: return START_NOT_STICKY
                start(host, port, fp, tok)
            }
            ACTION_STOP -> stop()
        }
        return START_NOT_STICKY
    }

    private fun start(host: String, port: Int, fp: String, token: String) {
        teardown() // старое убираем, но сам сервис не останавливаем: он только что запущен
        val p = LocalProxy(host, port, fp, token) { s -> protect(s) }
        val local = p.start(LOCAL_PORT)
        proxy = p
        val b = Builder().setSession("YANDI").addAddress("10.77.77.2", 32).addRoute("10.77.77.0", 24).setMtu(1400)
        if (Build.VERSION.SDK_INT >= 29) b.setHttpProxy(ProxyInfo.buildDirectProxy("127.0.0.1", local))
        tun = b.establish()
        running = tun != null
        Log.i(TAG, "VPN (прокси-режим) running=$running, локальный прокси на $local")
    }

    private fun teardown() {
        running = false
        proxy?.stop(); proxy = null
        runCatching { tun?.close() }; tun = null
    }

    private fun stop() {
        teardown()
        stopSelf()
    }

    override fun onRevoke() { teardown(); super.onRevoke() }
    override fun onDestroy() { teardown(); super.onDestroy() }
}

/** Прокси на 127.0.0.1: HTTP CONNECT / обычный HTTP / SOCKS5 -> TLS до компьютера -> `CONNECT цель`. */
class LocalProxy(
    private val host: String, private val port: Int, private val fingerprint: String, private val token: String,
    private val protect: (Socket) -> Boolean,
) {
    @Volatile private var server: ServerSocket? = null

    fun start(preferred: Int): Int {
        val s = try { ServerSocket(preferred, 64, InetAddress.getByName("127.0.0.1")) }
                catch (_: Exception) { ServerSocket(0, 64, InetAddress.getByName("127.0.0.1")) }
        server = s
        thread(name = "yandi-proxy-accept", isDaemon = true) {
            while (!s.isClosed) {
                val c = try { s.accept() } catch (_: Exception) { break }
                thread(name = "yandi-proxy-conn", isDaemon = true) { runCatching { handle(c) }; runCatching { c.close() } }
            }
        }
        return s.localPort
    }

    fun stop() { runCatching { server?.close() }; server = null }

    private fun handle(c: Socket) {
        c.tcpNoDelay = true
        val cin = BufferedInputStream(c.getInputStream(), 8192)
        val cout = c.getOutputStream()
        cin.mark(1)
        val first = cin.read()
        if (first < 0) return
        cin.reset()
        if (first == 5) socks5(c, cin, cout) else http(c, cin, cout)
    }

    // ── SOCKS5 (без пароля: слушаем только на самом телефоне) ──────────────────
    private fun socks5(c: Socket, cin: InputStream, cout: OutputStream) {
        cin.read(); val n = cin.read(); skip(cin, n)
        cout.write(byteArrayOf(5, 0)); cout.flush()
        val h = readN(cin, 4)
        if (h[1].toInt() != 1) { cout.write(byteArrayOf(5, 7, 0, 1, 0, 0, 0, 0, 0, 0)); return }
        val target = when (h[3].toInt()) {
            1 -> { val a = readN(cin, 4); val p = readN(cin, 2); InetAddress.getByAddress(a).hostAddress + ":" + port16(p) }
            3 -> { val l = cin.read(); val name = String(readN(cin, l)); val p = readN(cin, 2); "$name:${port16(p)}" }
            4 -> { val a = readN(cin, 16); val p = readN(cin, 2); "[" + InetAddress.getByAddress(a).hostAddress + "]:" + port16(p) }
            else -> return
        }
        val up = try { openTunnel(target) } catch (e: Exception) {
            cout.write(byteArrayOf(5, 5, 0, 1, 0, 0, 0, 0, 0, 0)); return
        }
        cout.write(byteArrayOf(5, 0, 0, 1, 0, 0, 0, 0, 0, 0)); cout.flush()
        pump(c, up.first, up.second)
    }

    // ── HTTP: CONNECT или обычный запрос с полным адресом ──────────────────────
    private fun http(c: Socket, cin: InputStream, cout: OutputStream) {
        val head = readHead(cin) ?: return
        val lines = head.split("\r\n")
        val parts = lines[0].split(" ")
        if (parts.size < 3) return
        if (parts[0].equals("CONNECT", true)) {
            val up = try { openTunnel(parts[1]) } catch (e: Exception) {
                cout.write("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n".toByteArray()); return
            }
            cout.write("HTTP/1.1 200 Connection established\r\n\r\n".toByteArray()); cout.flush()
            pump(c, up.first, up.second)
            return
        }
        // обычный HTTP: http://host[:port]/path -> соединение до host:port и запрос с путём
        val url = parts[1]
        if (!url.startsWith("http://")) { cout.write("HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".toByteArray()); return }
        val rest = url.removePrefix("http://")
        val slash = rest.indexOf('/')
        val hostPort = if (slash < 0) rest else rest.substring(0, slash)
        val path = if (slash < 0) "/" else rest.substring(slash)
        val target = if (hostPort.contains(':')) hostPort else "$hostPort:80"
        val up = try { openTunnel(target) } catch (e: Exception) {
            cout.write("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n".toByteArray()); return
        }
        val fixed = StringBuilder("${parts[0]} $path ${parts[2]}\r\n")
        for (l in lines.drop(1)) if (l.isNotEmpty() && !l.startsWith("Proxy-", true)) fixed.append(l).append("\r\n")
        fixed.append("\r\n")
        up.second.write(fixed.toString().toByteArray()); up.second.flush()
        pump(c, up.first, up.second)
    }

    // ── канал до компьютера ─────────────────────────────────────────────────────
    private fun openTunnel(target: String): Pair<InputStream, OutputStream> {
        val raw = Socket()
        protect(raw)
        raw.connect(InetSocketAddress(host, port), 10_000)
        raw.tcpNoDelay = true
        val ctx = SSLContext.getInstance("TLS")
        ctx.init(null, arrayOf(object : X509TrustManager {
            override fun checkClientTrusted(chain: Array<X509Certificate>, a: String) {}
            override fun checkServerTrusted(chain: Array<X509Certificate>, a: String) {
                val fp = MessageDigest.getInstance("SHA-256").digest(chain[0].encoded).joinToString("") { "%02x".format(it) }
                if (fingerprint.isNotEmpty() && !fp.equals(fingerprint, true)) throw CertificateException("отпечаток сертификата не совпал")
            }
            override fun getAcceptedIssuers(): Array<X509Certificate> = arrayOf()
        }), null)
        val tls = ctx.socketFactory.createSocket(raw, host, port, true) as javax.net.ssl.SSLSocket
        tls.startHandshake()
        val out = tls.outputStream
        out.write("CONNECT $target HTTP/1.1\r\nHost: $target\r\nProxy-Authorization: Bearer $token\r\n\r\n".toByteArray()); out.flush()
        val inp = BufferedInputStream(tls.inputStream, 16384)
        val head = readHead(inp) ?: throw java.io.IOException("нет ответа")
        if (!head.startsWith("HTTP/1.1 200")) throw java.io.IOException(head.lineSequence().first())
        YandiVpnService.connections.incrementAndGet()
        return Pair(inp, out)
    }

    private fun pump(c: Socket, upIn: InputStream, upOut: OutputStream) {
        val t = thread(isDaemon = true) {
            runCatching { copy(upIn, c.getOutputStream(), YandiVpnService.bytesDown) }
            runCatching { c.shutdownOutput() }
        }
        runCatching { copy(c.getInputStream(), upOut, YandiVpnService.bytesUp) }
        runCatching { upOut.flush() }
        t.join(300_000)
    }

    private fun copy(i: InputStream, o: OutputStream, counter: AtomicLong) {
        val b = ByteArray(16 * 1024)
        while (true) {
            val n = i.read(b); if (n < 0) break
            o.write(b, 0, n); o.flush(); counter.addAndGet(n.toLong())
        }
    }

    private fun readHead(i: InputStream): String? {
        val buf = ByteArray(16384); var n = 0
        while (n < buf.size) {
            val b = i.read(); if (b < 0) return null
            buf[n++] = b.toByte()
            if (n >= 4 && buf[n-4] == 13.toByte() && buf[n-3] == 10.toByte() && buf[n-2] == 13.toByte() && buf[n-1] == 10.toByte()) {
                return String(buf, 0, n - 4, Charsets.ISO_8859_1)
            }
        }
        return null
    }

    private fun readN(i: InputStream, n: Int): ByteArray {
        val b = ByteArray(n); var o = 0
        while (o < n) { val r = i.read(b, o, n - o); if (r < 0) throw java.io.EOFException(); o += r }
        return b
    }
    private fun skip(i: InputStream, n: Int) { readN(i, n) }
    private fun port16(p: ByteArray) = ((p[0].toInt() and 0xFF) shl 8) or (p[1].toInt() and 0xFF)
}
