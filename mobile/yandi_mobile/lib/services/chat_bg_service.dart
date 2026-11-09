import 'package:flutter/services.dart';
import '../models/trusted_node.dart';
import 'dart:typed_data';

/// Управляет фоновым Android-сервисом для WebSocket-соединения с нодой.
/// Сервис держит соединение живым когда приложение свёрнуто и показывает
/// уведомления о входящих сообщениях — без Google, без FCM.
class ChatBgService {
  static const _channel = MethodChannel('com.yandi/chat');

  Future<void> setMessageSoundEnabled(bool enabled) async {
    try {
      await _channel.invokeMethod<void>('messageSound', {'enabled': enabled});
    } on PlatformException catch (_) {
    } on MissingPluginException catch (_) {}
  }

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
      print('ChatBgService start error: ${e.runtimeType}');
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

  Future<bool> batteryOptimizationIgnored() async {
    try {
      return await _channel.invokeMethod<bool>('batteryOptimizationIgnored') ?? false;
    } on PlatformException catch (_) { return false; }
  }

  Future<bool> requestBatteryOptimizationException() async {
    try {
      return await _channel.invokeMethod<bool>('requestBatteryOptimizationException') ?? false;
    } on PlatformException catch (_) { return false; }
  }

  Future<List<Uint8List>> drainPendingLive() async {
    try {
      final raw = await _channel.invokeMethod<List<dynamic>>('drainPendingLive');
      return (raw ?? const <dynamic>[]).map<Uint8List?>((item) {
        if (item is Uint8List) return item;
        if (item is List) return Uint8List.fromList(item.whereType<int>().toList());
        return null;
      }).whereType<Uint8List>().toList();
    } on PlatformException catch (_) {
      return const <Uint8List>[];
    } on MissingPluginException catch (_) {
      return const <Uint8List>[];
    }
  }
}
