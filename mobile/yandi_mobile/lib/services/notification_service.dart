import 'vault_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import '../l10n_core.dart';

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
    final launch = await _plugin.getNotificationAppLaunchDetails();
    if (launch?.didNotificationLaunchApp == true) {
      final response = launch?.notificationResponse;
      if (response != null) _onResponse(response);
    }

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
    pendingOpenPeerId = peerId;   // прежде навигации: Home может прочитать значение сразу

    final nav = navigatorKey.currentState;
    // пока приложение заблокировано, уведомление не открывает чат в обход входа: после ввода пароля главный экран сам откроет нужный
    if (nav != null && VaultService.instance.unlocked) {
      // Приложение живо — открываем чат напрямую
      nav.pushNamed('/home');
      // HomeScreen заметит pendingOpenPeerId и откроет нужный чат
    }
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
        tr('Сообщения'),
        channelDescription: tr('Входящие сообщения YANDI'),
        importance:   Importance.high,
        priority:     Priority.high,
        icon:         '@drawable/ic_notification',
        color:        const Color(0xFF00E5FF),
        playSound:    false,  // звук воспроизводит AppState через audioplayers
        enableVibration: true,
        // Общая группа для всех: идентификатор собеседника в системные данные уведомлений не попадает
        groupKey:        'yandi_messages',
        setAsGroupSummary: false,
      ),
    );
    // Уведомление нейтральное: ни имени, ни текста. Всё, что попало в системные уведомления, остаётся там после блокировки приложения
    // и видно другим программам с доступом к уведомлениям; содержимое показывается только внутри открытого приложения.
    await _plugin.show(
      _stableId(fromPeerId),
      'YANDI',
      tr('Новое сообщение'),
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
    final details = NotificationDetails(
      android: AndroidNotificationDetails(
        'yandi_files',
        tr('Файлы'),
        channelDescription: tr('Входящие файлы YANDI'),
        importance: Importance.high,
        priority:   Priority.high,
        icon:       '@drawable/ic_notification',
        color:      Color(0xFF00E5FF),
      ),
    );
    await _plugin.show(
      _stableId('file_$fromPeerId'),
      'YANDI',
      tr('Новый файл'),
      details,
      payload: fromPeerId,
    );
  }

  // ── Входящий звонок: на весь экран, если телефон заблокирован ──────────────

  static const int _callId = 0x0CA11;

  static Future<void> showIncomingCall({required String fromPeerId, required String displayName, required bool video}) async {
    final details = NotificationDetails(
      android: AndroidNotificationDetails(
        'yandi_calls',
        tr('Звонки'),
        channelDescription: tr('Входящие звонки и видеозвонки YANDI'),
        importance: Importance.max,
        priority:   Priority.max,
        icon:       '@drawable/ic_notification',
        color:      const Color(0xFF00E5FF),
        category:   AndroidNotificationCategory.call,
        fullScreenIntent: true,
        ongoing:    true,
        autoCancel: false,
        playSound:  false, // мелодию играет сам звонок
        visibility: NotificationVisibility.public,
        timeoutAfter: 45000,
      ),
    );
    await _plugin.show(_callId, 'YANDI', video ? tr('Входящий видеозвонок') : tr('Входящий звонок'), details, payload: fromPeerId);
  }

  static Future<void> cancelIncomingCall() async {
    await _plugin.cancel(_callId);
  }

  // ── Убрать уведомления для конкретного чата ─────────────────────────────────

  static Future<void> cancelForPeer(String fromPeerId) async {
    await _plugin.cancel(_stableId(fromPeerId));
  }

  /// Снять все уведомления (события просмотрены) — гаснет и красный значок у иконки приложения.
  static Future<void> cancelAll() async {
    await _plugin.cancelAll();
  }

  static int _stableId(String key) => key.hashCode.abs() % 0x7FFFFFFF;
}
