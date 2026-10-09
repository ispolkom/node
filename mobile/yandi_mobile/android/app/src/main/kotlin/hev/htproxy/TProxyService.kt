package hev.htproxy

/**
 * Мост к hev-socks5-tunnel. Имя пакета и класса зашиты в нативной части (JNI_OnLoad регистрирует методы именно на hev/htproxy/TProxyService),
 * поэтому класс лежит здесь, а не в com.yandi.
 */
object TProxyService {
    /** true, если нативная библиотека загрузилась (на неподдерживаемой архитектуре остаётся прокси-режим). */
    val available: Boolean = try { System.loadLibrary("hev-socks5-tunnel"); true } catch (_: Throwable) { false }

    external fun TProxyStartService(configPath: String, fd: Int): Boolean
    external fun TProxyStopService(): Boolean
    external fun TProxyIsRunning(): Boolean
    external fun TProxyGetStats(): LongArray
}
