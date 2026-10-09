package com.yandi.yandi_mobile

import android.app.Activity
import android.content.ContentValues
import android.content.Intent
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import java.io.File
import android.net.VpnService
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {

    private var pendingPrepare: MethodChannel.Result? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val messenger = flutterEngine.dartExecutor.binaryMessenger

        MethodChannel(messenger, "com.yandi.yandi_mobile/vpn").setMethodCallHandler { call, result -> vpn(call, result) }
        // в прокси-режиме пакеты через приложение не идут; канал нужен, чтобы подписка из Dart не падала
        EventChannel(messenger, "com.yandi.yandi_mobile/vpn_events").setStreamHandler(object : EventChannel.StreamHandler {
            override fun onListen(args: Any?, sink: EventChannel.EventSink?) {}
            override fun onCancel(args: Any?) {}
        })

        MethodChannel(messenger, "com.yandi.yandi_mobile/files").setMethodCallHandler { call, result ->
            when (call.method) {
                "saveToDownloads" -> {
                    val path = call.argument<String>("path")
                    val name = call.argument<String>("name") ?: "file"
                    val mime = call.argument<String>("mime") ?: "application/octet-stream"
                    if (path == null) { result.error("arg", "нет пути", null); return@setMethodCallHandler }
                    Thread {
                        val r = runCatching { saveToDownloads(File(path), name, mime) }
                        runOnUiThread { r.fold({ result.success(it) }, { result.error("save", it.message, null) }) }
                    }.start()
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

    /** Копия файла в общую папку «Загрузки»: через MediaStore на Android 10+, обычной записью на старых. */
    private fun saveToDownloads(src: File, name: String, mime: String): String {
        if (Build.VERSION.SDK_INT >= 29) {
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, name)
                put(MediaStore.Downloads.MIME_TYPE, mime)
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
            val uri = contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: error("не удалось создать файл")
            contentResolver.openOutputStream(uri)!!.use { out -> src.inputStream().use { it.copyTo(out) } }
            values.clear(); values.put(MediaStore.Downloads.IS_PENDING, 0)
            contentResolver.update(uri, values, null, null)
            return uri.toString()
        }
        val dir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).apply { mkdirs() }
        var dst = File(dir, name)
        var n = 1
        while (dst.exists()) { dst = File(dir, "${name.substringBeforeLast('.', name)} ($n)${if (name.contains('.')) "." + name.substringAfterLast('.') else ""}"); n++ }
        src.copyTo(dst)
        return dst.absolutePath
    }
}
