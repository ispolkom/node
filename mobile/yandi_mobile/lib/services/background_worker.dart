import 'package:workmanager/workmanager.dart';

const _taskDiscovery = 'com.yandi.discovery';

/// Фоновое обновление списка нод раз в сутки отключено: оно открывало базу приложения без пароля, а база теперь зашифрована ключом,
/// которого в фоне нет. Список нод обновляется при каждом запуске приложения (`NodeDiscovery.discoverOnStartup`).
/// Прежнее зарегистрированное задание на обновлённых установках снимается.
Future<void> initBackgroundWorker() async {
  await Workmanager().initialize(_callbackDispatcher);
  await Workmanager().cancelByUniqueName(_taskDiscovery);
}

/// Top-level callback — WorkManager вызывает его в изолированном Dart-потоке. Базу не трогает.
@pragma('vm:entry-point')
void _callbackDispatcher() {
  Workmanager().executeTask((taskName, inputData) async => Future.value(true));
}
