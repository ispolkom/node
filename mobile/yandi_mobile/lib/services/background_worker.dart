import 'package:workmanager/workmanager.dart';
import 'node_discovery.dart';
import 'node_manager.dart';
import 'storage_service.dart';

const _taskDiscovery = 'com.yandi.discovery';

/// Инициализирует WorkManager при старте приложения.
/// Регистрирует ежедневную фоновую задачу обновления списка нод.
Future<void> initBackgroundWorker() async {
  await Workmanager().initialize(
    _callbackDispatcher,
    isInDebugMode: false,
  );

  // Запускаем periodic-задачу: раз в сутки, только на зарядке и WiFi
  await Workmanager().registerPeriodicTask(
    _taskDiscovery,
    _taskDiscovery,
    frequency: const Duration(hours: 24),
    constraints: Constraints(
      networkType:          NetworkType.unmetered, // только WiFi
      requiresCharging:     true,                  // только на зарядке
      requiresBatteryNotLow: true,
    ),
    existingWorkPolicy: ExistingWorkPolicy.keep,
  );
}

/// Top-level callback — WorkManager вызывает его в изолированном Dart-потоке.
/// Не может обращаться к UI или Provider — только к сервисам напрямую.
@pragma('vm:entry-point')
void _callbackDispatcher() {
  Workmanager().executeTask((taskName, inputData) async {
    if (taskName != _taskDiscovery) return Future.value(true);

    try {
      await StorageService.init();
      final manager = NodeManager();
      await manager.load();
      await NodeDiscovery.backgroundRefresh(manager);
    } catch (_) {
      // Не падаем — WorkManager попробует снова через сутки
    }

    return Future.value(true);
  });
}
