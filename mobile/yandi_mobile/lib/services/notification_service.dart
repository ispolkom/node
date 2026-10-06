import 'package:flutter/material.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

/// Сервис локальных уведомлений (без Firebase/Google).
///
/// Канал messages — входящие сообщения (высокий приоритет).
/// Канал files    — предложения файлов.
///
/// Тап по уведомлению → открывает нужный чат через [navigatorKey].
class NotificationService {
  NotificationService._();

  static final _plugin = FlutterLocalNotificationsPlugin();

  /// GlobalKey передаётся в MaterialApp.navigatorKey — позволяет навигировать
  /// из любого места без BuildContext.
  static final navigatorKey = GlobalKey<NavigatorState>();

  /// peer_id чата который нужно открыть после перехода из убитого приложения.
  static String? pendingOpenPeerId;

  static Future<void> init() async {
    const androidSettings =
        AndroidInitializationSettings('@drawable/ic_notification');
    const settings = InitializationSettings(android: androidSettings);

    await _plugin.initialize(
      settings,
      onDidReceiveNotificationResponse:         _onResponse,
      onDidReceiveBackgroundNotificationResponse: _onResponse,
    );

    // Запрашиваем разрешение (Android 13+).
    await _plugin
        .resolvePlatformSpecificImplementation<
            AndroidFlutterLocalNotificationsPlugin>()
        ?.requestNotificationsPermission();
  }

  @pragma('vm:entry-point')
  static void _onResponse(NotificationResponse res) {
    final peerId = res.payload;
    if (peerId == null || peerId.isEmpty) return;

    final nav = navigatorKey.currentState;
    if (nav != null) {
      // Приложение живо — открываем чат напрямую
      nav.pushNamed('/home');
      // HomeScreen заметит pendingOpenPeerId и откроет нужный чат
    }
    pendingOpenPeerId = peerId;
  }

  // ── Показать уведомление о сообщении ────────────────────────────────────────

  static Future<void> showMessage({
    required String fromPeerId,
    required String displayName,
    required String text,
  }) async {
    final details = NotificationDetails(
      android: AndroidNotificationDetails(
        'yandi_messages',
        'Сообщения',
        channelDescription: 'Входящие сообщения YANDI',
        importance:   Importance.high,
        priority:     Priority.high,
        icon:         '@drawable/ic_notification',
        color:        const Color(0xFF00E5FF),
        playSound:    false,  // звук воспроизводит AppState через audioplayers
        enableVibration: true,
        // Группируем по отправителю — несколько сообщений в одно уведомление
        groupKey:        'peer_$fromPeerId',
        setAsGroupSummary: false,
      ),
    );
    await _plugin.show(
      _stableId(fromPeerId),
      displayName,
      text.length > 80 ? '${text.substring(0, 80)}…' : text,
      details,
      payload: fromPeerId,
    );
  }

  // ── Показать уведомление о входящем файле ───────────────────────────────────

  static Future<void> showFileOffer({
    required String fromPeerId,
    required String displayName,
    required String fileName,
  }) async {
    const details = NotificationDetails(
      android: AndroidNotificationDetails(
        'yandi_files',
        'Файлы',
        channelDescription: 'Входящие файлы YANDI',
        importance: Importance.high,
        priority:   Priority.high,
        icon:       '@drawable/ic_notification',
        color:      Color(0xFF00E5FF),
      ),
    );
    await _plugin.show(
      _stableId('file_$fromPeerId'),
      displayName,
      '📎 $fileName',
      details,
      payload: fromPeerId,
    );
  }

  // ── Убрать уведомления для конкретного чата ─────────────────────────────────

  static Future<void> cancelForPeer(String fromPeerId) async {
    await _plugin.cancel(_stableId(fromPeerId));
  }

  static int _stableId(String key) => key.hashCode.abs() % 0x7FFFFFFF;
}
