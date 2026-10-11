import 'dart:async';
import 'package:flutter/widgets.dart';
import 'package:provider/provider.dart';
import 'app_state.dart';
import 'vault_service.dart';

/// Блокировка приложения: через минуту после ухода в фон (или при возврате, если минута прошла) соединения закрываются, память
/// очищается, ключ стирается, база закрывается, и открывается экран ввода пароля. Во время звонка блокировка откладывается.
class VaultLockObserver with WidgetsBindingObserver {
  VaultLockObserver(this.navigatorKey) {
    instance = this;
  }
  final GlobalKey<NavigatorState> navigatorKey;

  /// Для кнопки «Заблокировать сейчас».
  static VaultLockObserver? instance;

  static const lockAfter = Duration(seconds: 60);
  DateTime? _backgroundedAt;
  Timer? _timer;
  bool _locking = false;

  void attach() => WidgetsBinding.instance.addObserver(this);

  @override
  void didChangeAppLifecycleState(AppLifecycleState s) {
    if (s == AppLifecycleState.paused) {
      _backgroundedAt = DateTime.now();
      _timer?.cancel();
      _timer = Timer(lockAfter, lockNow);
    } else if (s == AppLifecycleState.resumed) {
      _timer?.cancel();
      final bg = _backgroundedAt;
      _backgroundedAt = null;
      if (bg != null && DateTime.now().difference(bg) >= lockAfter) lockNow();
    }
  }

  Future<void> lockNow() async {
    if (_locking || !VaultService.instance.active) return;
    final ctx = navigatorKey.currentContext;
    if (ctx == null) return;
    final state = ctx.read<AppState>();
    if (state.calls.busy) {
      _timer?.cancel();
      _timer = Timer(const Duration(seconds: 30), lockNow);   // идёт звонок: заблокируем после него
      return;
    }
    _locking = true;
    try {
      // что бы ни сорвалось при остановке служб, ключи стираются и открывается экран входа
      try { await state.lockDown(); } catch (_) {}
    } finally {
      try { await VaultService.instance.lock(); } catch (_) {}
      navigatorKey.currentState?.pushNamedAndRemoveUntil('/vault-unlock', (_) => false);
      _locking = false;
    }
  }
}
