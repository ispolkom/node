import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/services.dart';

/// VPN Service для Android
/// Управляет VpnService через MethodChannel
class YandiVpnService {
  static const _channel = MethodChannel('com.yandi.yandi_mobile/vpn');
  static const _eventChannel = EventChannel('com.yandi.yandi_mobile/vpn_events');

  static final YandiVpnService _instance = YandiVpnService._internal();
  factory YandiVpnService() => _instance;
  YandiVpnService._internal();

  StreamSubscription? _packetSubscription;
  final _packetController = StreamController<Uint8List>.broadcast();

  /// Поток входящих пакетов из VPN
  Stream<Uint8List> get packets => _packetController.stream;

  /// Проверить, есть ли разрешение на VPN
  Future<bool> isVpnPrepared() async {
    final result = await _channel.invokeMethod<bool>('isVpnPrepared');
    return result ?? false;
  }

  /// Запросить разрешение на VPN (показывает системный диалог)
  Future<bool> prepareVpn() async {
    final result = await _channel.invokeMethod<bool>('prepareVpn');
    return result ?? false;
  }

  /// Запустить VPN
  /// [serverAddress] - адрес YANDI ноды
  /// [serverPort] - порт ноды (10000 для proxy)
  Future<bool> startVpn({
    required String serverAddress,
    required int serverPort,
  }) async {
    // Сначала проверяем разрешение
    if (!await isVpnPrepared()) {
      final prepared = await prepareVpn();
      if (!prepared) {
        return false;
      }
    }

    // Запускаем VPN
    final result = await _channel.invokeMethod<bool>('startVpn', {
      'serverAddress': serverAddress,
      'serverPort': serverPort,
    });

    if (result == true) {
      _startPacketListener();
    }

    return result ?? false;
  }

  /// Остановить VPN
  Future<void> stopVpn() async {
    _packetSubscription?.cancel();
    _packetSubscription = null;
    await _channel.invokeMethod('stopVpn');
  }

  /// Проверить, запущен ли VPN
  Future<bool> isVpnRunning() async {
    final result = await _channel.invokeMethod<bool>('isVpnRunning');
    return result ?? false;
  }

  /// Отправить пакет в VPN интерфейс
  Future<void> writePacket(Uint8List packet) async {
    await _channel.invokeMethod('writePacket', {'packet': packet});
  }

  /// Получить статистику VPN
  Future<Map<String, dynamic>?> getStats() async {
    final result = await _channel.invokeMethod<Map>('getStats');
    return result?.cast<String, dynamic>();
  }

  /// Запустить слушатель пакетов
  void _startPacketListener() {
    _packetSubscription?.cancel();
    _packetSubscription = _eventChannel.receiveBroadcastStream().listen(
      (event) {
        if (event is List) {
          _packetController.add(Uint8List.fromList(event.cast<int>()));
        }
      },
      onError: (error) {
        print('[VPN] Packet listener error: $error');
      },
    );
  }

  Future<bool> start({
    required String socksHost,
    required int    socksPort,
    String? socksUser,
    String? socksPass,
  }) => startVpn(serverAddress: socksHost, serverPort: socksPort);

  Future<void> stop() => stopVpn();

  /// Освободить ресурсы
  void dispose() {
    _packetSubscription?.cancel();
    _packetController.close();
  }
}

extension YandiVpnServiceExt on YandiVpnService {
  Future<bool> start({
    required String socksHost,
    required int    socksPort,
    String? socksUser,
    String? socksPass,
  }) => startVpn(serverAddress: socksHost, serverPort: socksPort);

  Future<void> stop() => stopVpn();
}
