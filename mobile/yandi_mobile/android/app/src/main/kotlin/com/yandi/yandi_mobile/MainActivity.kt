package com.yandi.yandi_mobile

import android.app.Activity
import android.content.Intent
import android.os.Build
import java.io.File
import android.net.VpnService
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {

    private var pendingPrepare: MethodChannel.Result? = null
    private var linkChannel: MethodChannel? = null
    private var initialLink: String? = null

    /// ссылка-приглашение yandi://invite?d=…: отдаём её только нашей схеме и хосту, остальное игнорируем
    private fun inviteLink(i: Intent?): String? {
        val d = i?.data ?: return null
        return if (i.action == Intent.ACTION_VIEW && d.scheme == "yandi" && d.host == "invite") d.toString() else null
    }

    override fun onCreate(savedInstanceState: android.os.Bundle?) {
        // полная защита экрана (решение владельца 2026-10-21): до первого кадра — без снимков и записи экрана, демонстрация
        // экрана и превью в списке приложений пустые
        window.setFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE, android.view.WindowManager.LayoutParams.FLAG_SECURE)
        super.onCreate(savedInstanceState)
        // касания сквозь чужое окно поверх (подмена нажатий) не принимаются
        window.decorView.filterTouchesWhenObscured = true
        // Android 14+: содержимое окна не отдаётся службам специальных возможностей, кроме настоящих средств доступности
        // (экранный диктор), — сторонние «читалки экрана» текст переписки не получат
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            window.decorView.setAccessibilityDataSensitive(android.view.View.ACCESSIBILITY_DATA_SENSITIVE_YES)
        }
        initialLink = inviteLink(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        inviteLink(intent)?.let { linkChannel?.invokeMethod("link", it) }
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val messenger = flutterEngine.dartExecutor.binaryMessenger

        linkChannel = MethodChannel(messenger, "com.yandi.yandi_mobile/link").also { ch ->
            ch.setMethodCallHandler { call, result ->
                if (call.method == "initial") { result.success(initialLink); initialLink = null } else result.notImplemented()
            }
        }
        MethodChannel(messenger, "com.yandi.yandi_mobile/vpn").setMethodCallHandler { call, result -> vpn(call, result) }
        // в прокси-режиме пакеты через приложение не идут; канал нужен, чтобы подписка из Dart не падала
        EventChannel(messenger, "com.yandi.yandi_mobile/vpn_events").setStreamHandler(object : EventChannel.StreamHandler {
            override fun onListen(args: Any?, sink: EventChannel.EventSink?) {}
            override fun onCancel(args: Any?) {}
        })

        MethodChannel(messenger, "com.yandi.yandi_mobile/files").setMethodCallHandler { call, result ->
            when (call.method) {
                // затирание файла поверх его байтов и удаление (FileWipe); вызывается из Dart уже после мгновенного скрытия файла из списка
                "wipe" -> {
                    val path = call.argument<String>("path")
                    if (path == null) { result.error("arg", "нет пути", null); return@setMethodCallHandler }
                    Thread {
                        val ok = runCatching { FileWipe.wipe(File(path)) }.getOrDefault(false)
                        runOnUiThread { result.success(ok) }
                    }.start()
                }
                // кадр видео для превью в чате: источник — адрес на 127.0.0.1 (видео расшифровывается в памяти, файла на диске нет)
                "videoFrame" -> {
                    val url = call.argument<String>("url")
                    val ms = call.argument<Int>("ms") ?: 0
                    val maxW = call.argument<Int>("maxWidth") ?: 480
                    if (url == null) { result.error("arg", "нет адреса", null); return@setMethodCallHandler }
                    Thread {
                        val r = runCatching {
                            val mmr = android.media.MediaMetadataRetriever()
                            try {
                                mmr.setDataSource(url, HashMap<String, String>())
                                var bmp = mmr.getFrameAtTime(ms.toLong() * 1000, android.media.MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
                                    ?: mmr.getFrameAtTime(0)
                                    ?: throw IllegalStateException("нет кадра")
                                if (bmp.width > maxW) {
                                    val h = (bmp.height.toLong() * maxW / bmp.width).toInt().coerceAtLeast(1)
                                    bmp = android.graphics.Bitmap.createScaledBitmap(bmp, maxW, h, true)
                                }
                                val out = java.io.ByteArrayOutputStream()
                                bmp.compress(android.graphics.Bitmap.CompressFormat.JPEG, 72, out)
                                out.toByteArray()
                            } finally {
                                runCatching { mmr.release() }
                            }
                        }
                        runOnUiThread { r.fold({ result.success(it) }, { result.error("frame", it.message, null) }) }
                    }.start()
                }
                // системное окно удаления этого приложения (после «Удалить всё» с паролем)
                "uninstall" -> {
                    runCatching {
                        startActivity(Intent(Intent.ACTION_DELETE, android.net.Uri.parse("package:$packageName")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }
                    result.success(true)
                }
                // переслать текст (адрес кошелька проекта) через системное окно «Поделиться»: в другое приложение, мессенджер, кошелёк
                "shareText" -> {
                    val text = call.argument<String>("text")
                    if (text.isNullOrEmpty() || text.length > 4096) { result.error("arg", "нет текста", null); return@setMethodCallHandler }
                    runCatching {
                        val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
                        startActivity(Intent.createChooser(send, "Отправить адрес").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }.fold({ result.success(true) }, { result.error("share", it.message, null) })
                }
                // окно закрыто от скриншотов, записи экрана и превью в списке приложений (экраны ввода пароля и секретных слов)
                "secure" -> {
                    val on = call.argument<Boolean>("on") ?: false
                    runOnUiThread {
                        // защита включена всегда: прежний выключатель экранов пароля больше не снимает её
                        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE)
                        if (!on) { /* без снятия */ }
                    }
                    result.success(true)
                }
                else -> result.notImplemented()
            }
        }

        MethodChannel(messenger, "com.yandi/chat").setMethodCallHandler { call, result ->
            when (call.method) {
                "start" -> {
                    val i = Intent(this, YandiChatService::class.java).setAction(YandiChatService.ACTION_START)
                        .putExtra("host", call.argument<String>("host"))
                        .putExtra("port", call.argument<Int>("port") ?: 443)
                        .putExtra("token", call.argument<String>("token"))
                        .putExtra("fingerprint", call.argument<String>("fingerprint") ?: "")
                    runCatching { startForegroundService(i) }.onFailure { startService(i) }
                    result.success(true)
                }
                "stop" -> { startService(Intent(this, YandiChatService::class.java).setAction(YandiChatService.ACTION_STOP)); result.success(true) }
                "isRunning" -> result.success(YandiChatService.running)
                else -> result.notImplemented()
            }
        }
    }

    private fun vpn(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "isVpnPrepared" -> result.success(VpnService.prepare(this) == null)
            "prepareVpn" -> {
                val i = VpnService.prepare(this)
                if (i == null) result.success(true) else { pendingPrepare = result; startActivityForResult(i, 7001) }
            }
            "startVpn" -> {
                val i = Intent(this, YandiVpnService::class.java).setAction(YandiVpnService.ACTION_START)
                    .putExtra("host", call.argument<String>("serverAddress"))
                    .putExtra("port", call.argument<Int>("serverPort") ?: 443)
                    .putExtra("fingerprint", call.argument<String>("fingerprint") ?: "")
                    .putExtra("token", call.argument<String>("token"))
                startService(i)
                result.success(true)
            }
            "stopVpn" -> { startService(Intent(this, YandiVpnService::class.java).setAction(YandiVpnService.ACTION_STOP)); result.success(true) }
            "isVpnRunning" -> result.success(YandiVpnService.running)
            "getStats" -> result.success(mapOf("bytesUp" to YandiVpnService.bytesUp.get(), "bytesDown" to YandiVpnService.bytesDown.get(), "connections" to YandiVpnService.connections.get()))
            "writePacket" -> result.success(null)
            else -> result.notImplemented()
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == 7001) {
            pendingPrepare?.success(resultCode == Activity.RESULT_OK)
            pendingPrepare = null
        } else {
            @Suppress("DEPRECATION")
            super.onActivityResult(requestCode, resultCode, data)
        }
    }
}
