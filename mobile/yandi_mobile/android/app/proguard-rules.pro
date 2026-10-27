# Правила сжатия кода (R8) для release-сборки YANDI.

# flutter_local_notifications 17.x uses Gson reflection for its notification cache.
# R8 full mode otherwise removes TypeToken's generic argument (runtime crash on cancel).
-keepattributes Signature,InnerClasses,EnclosingMethod
-keep class com.google.gson.reflect.TypeToken { *; }
-keep class * extends com.google.gson.reflect.TypeToken { *; }
-keep class com.dexterous.flutterlocalnotifications.models.** { *; }

# Нативная часть VPN (hev-socks5-tunnel): JNI ищет методы по имени класса и метода — не переименовывать.
-keep class hev.htproxy.** { *; }
-keepclasseswithmembernames class * { native <methods>; }

# WebRTC (звонки): нативный код вызывает Java-классы по именам.
-keep class org.webrtc.** { *; }
-dontwarn org.webrtc.**
-keep class com.cloudwebrtc.webrtc.** { *; }

# Свои классы, которые вызываются из Flutter по каналам и из манифеста.
-keep class com.yandi.yandi_mobile.** { *; }

# Зашифрованная база SQLCipher: нативный код вызывает Java-классы по именам (библиотека приносит и свои правила — это запас).
-keep class net.zetetic.database.** { *; }
-dontwarn net.zetetic.database.**
-keep class com.davidmartos96.sqflite_sqlcipher.** { *; }

# Защищённое хранилище (EncryptedSharedPreferences / Tink).
-keep class com.google.crypto.tink.** { *; }
-dontwarn com.google.crypto.tink.**
-dontwarn com.google.errorprone.annotations.**
-dontwarn javax.annotation.**

# WorkManager и Play Core (отложенные компоненты Flutter) — без предупреждений, если их нет.
-dontwarn com.google.android.play.core.**
