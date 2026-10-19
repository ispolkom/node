import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:cryptography_flutter/cryptography_flutter.dart';
import 'package:provider/provider.dart';
import 'services/app_state.dart';
import 'services/background_worker.dart';
import 'services/notification_service.dart';
import 'services/vault_lock.dart';
import 'screens/splash_screen.dart';
import 'screens/vault_screens.dart';
import 'screens/pair_screen.dart';
import 'screens/home_screen.dart';
import 'screens/nodes_screen.dart';
import 'theme.dart';
import 'models/invite_payload.dart';
import 'services/invite_store.dart';
import 'services/vault_service.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();
  FlutterCryptography.enable();
  SystemChrome.setPreferredOrientations([DeviceOrientation.portraitUp]);
  VaultLockObserver(NotificationService.navigatorKey).attach();
  runApp(
    ChangeNotifierProvider(
      create: (_) => AppState(),
      child: const YandiApp(),
    ),
  );
  // Уведомления и фоновая задача не должны задерживать первый экран: запрос
  // разрешения может не вернуться, и приложение осталось бы чёрным.
  _initInBackground();
  _listenForInviteLinks();
}

/// Ссылка-приглашение `yandi://invite?d=…` (нажали в мессенджере или браузере): запоминаем её на час и, если закрытая среда открыта, сразу
/// показываем экран подключения, где нужно лишь ввести PIN. Иначе ссылка подхватится на экране подключения после входа.
void _listenForInviteLinks() {
  const ch = MethodChannel('com.yandi.yandi_mobile/link');
  Future<void> handle(String? link) async {
    final text = link == null ? null : inviteTextFromLink(link);
    if (text == null) return;
    final exp = inviteExpiryFromText(text);
    final expMs = exp != null ? exp * 1000 : DateTime.now().millisecondsSinceEpoch + 3600 * 1000;
    if (expMs <= DateTime.now().millisecondsSinceEpoch) return;
    await InviteStore.savePending(text: text, expMs: expMs);
    if (VaultService.instance.unlocked) {
      NotificationService.navigatorKey.currentState?.push(MaterialPageRoute(builder: (_) => PairScreen(initialPayload: text)));
    }
  }
  ch.setMethodCallHandler((call) async {
    if (call.method == 'link') await handle(call.arguments as String?);
  });
  ch.invokeMethod<String>('initial').then(handle).catchError((_) {});
}

Future<void> _initInBackground() async {
  try {
    await NotificationService.init().timeout(const Duration(seconds: 20));
  } catch (e) {
    debugPrint('notifications init: ${e.runtimeType}');
  }
  try {
    await initBackgroundWorker().timeout(const Duration(seconds: 20));
  } catch (e) {
    debugPrint('background worker init: ${e.runtimeType}');
  }
}

class YandiApp extends StatelessWidget {
  const YandiApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title:          'YANDI',
      debugShowCheckedModeBanner: false,
      theme:          AppTheme.dark,
      navigatorKey:   NotificationService.navigatorKey,
      initialRoute:   '/',
      onGenerateRoute: (settings) {
        if (settings.name == '/vault-unlock') {
          return MaterialPageRoute(settings: settings, builder: (_) => VaultUnlockScreen(justCreated: settings.arguments == true));
        }
        return null;
      },
      routes: {
        '/':                  (_) => const SplashScreen(),
        '/vault-setup':       (_) => const VaultSetupScreen(),
        '/pair':              (_) => const PairScreen(),
        '/home':              (_) => const _LifecycleWrapper(child: HomeScreen()),
        '/nodes':             (_) => const NodesScreen(),
      },
    );
  }
}

/// Оборачивает экран HomeScreen и отслеживает состояние экрана устройства
/// для адаптивного ping в WsService (30s когда активен, 90s когда фон).
class _LifecycleWrapper extends StatefulWidget {
  final Widget child;
  const _LifecycleWrapper({required this.child});

  @override
  State<_LifecycleWrapper> createState() => _LifecycleWrapperState();
}

class _LifecycleWrapperState extends State<_LifecycleWrapper>
    with WidgetsBindingObserver {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState s) {
    final appState = context.read<AppState>();
    if (s == AppLifecycleState.resumed) {
      appState.onScreenOn();
      NotificationService.cancelAll();   // вернулись в приложение — события просмотрены, значок гаснет
    } else if (s == AppLifecycleState.paused) {
      appState.onScreenOff();
    } else if (s == AppLifecycleState.inactive ||
               s == AppLifecycleState.detached) {
      appState.onScreenOff();
    }
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
