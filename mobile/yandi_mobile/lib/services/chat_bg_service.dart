import 'package:flutter/services.dart';
import '../models/trusted_node.dart';

/// Управляет фоновым Android-сервисом для WebSocket-соединения с нодой.
/// Сервис держит соединение живым когда приложение свёрнуто и показывает
/// уведомления о входящих сообщениях — без Google, без FCM.
class ChatBgService {
  static const _channel = MethodChannel('com.yandi/chat');

  Future<void> start(TrustedNode node) async {
    try {
      await _channel.invokeMethod('start', {
        'host':        node.host,
        'port':        node.port,
        'token':       node.token,
        'fingerprint': node.fingerprint,
      });
    } on PlatformException catch (e) {
      // Логируем, но не падаем — основной WS в приложении работает и так
      // ignore: avoid_print
      print('ChatBgService start error: $e');
    }
  }

  Future<void> stop() async {
    try {
      await _channel.invokeMethod('stop');
    } on PlatformException {
      // ignore
    }
  }

  Future<bool> isRunning() async {
    try {
      return await _channel.invokeMethod<bool>('isRunning') ?? false;
    } on PlatformException {
      return false;
    }
  }
}
