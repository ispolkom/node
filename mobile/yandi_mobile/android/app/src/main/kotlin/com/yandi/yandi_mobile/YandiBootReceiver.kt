package com.yandi.yandi_mobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build

class YandiBootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        if (intent?.action != Intent.ACTION_BOOT_COMPLETED && intent?.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        val config = YandiChatService.loadConfig(context) ?: return
        val start = Intent(context, YandiChatService::class.java).setAction(YandiChatService.ACTION_START)
            .putExtra("host", config[0]).putExtra("port", config[1].toInt()).putExtra("token", config[2]).putExtra("fingerprint", config[3])
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) context.startForegroundService(start) else context.startService(start)
    }
}
