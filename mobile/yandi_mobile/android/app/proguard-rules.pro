# Правила сжатия кода (R8) для release-сборки YANDI.

# Нативная часть VPN (hev-socks5-tunnel): JNI ищет методы по имени класса и метода — не переименовывать.
-keep class hev.htproxy.** { *; }
-keepclasseswithmembernames class * { native <methods>; }

# WebRTC (звонки): нативный код вызывает Java-классы по именам.
-keep class org.webrtc.** { *; }
-dontwarn org.webrtc.**
-keep class com.cloudwebrtc.webrtc.** { *; }

# Свои классы, которые вызываются из Flutter по каналам и из манифеста.
-keep class com.yandi.yandi_mobile.** { *; }

# Защищённое хранилище (EncryptedSharedPreferences / Tink).
-keep class com.google.crypto.tink.** { *; }
-dontwarn com.google.crypto.tink.**
-dontwarn com.google.errorprone.annotations.**
-dontwarn javax.annotation.**

# WorkManager и Play Core (отложенные компоненты Flutter) — без предупреждений, если их нет.
-dontwarn com.google.android.play.core.**
