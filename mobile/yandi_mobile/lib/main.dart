import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'services/app_state.dart';
import 'services/background_worker.dart';
import 'services/notification_service.dart';
import 'services/app_lock_service.dart';
import 'screens/splash_screen.dart';
import 'screens/create_account_screen.dart';
import 'screens/pair_screen.dart';
import 'screens/home_screen.dart';
import 'screens/nodes_screen.dart';
import 'theme.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();
  SystemChrome.setPreferredOrientations([DeviceOrientation.portraitUp]);
  await NotificationService.init();
  await initBackgroundWorker();
  runApp(
    ChangeNotifierProvider(
      create: (_) => AppState(),
      child: const YandiApp(),
    ),
  );
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
      routes: {
        '/':                  (_) => const SplashScreen(),
        '/create-account':    (_) => const CreateAccountScreen(),
        '/restore-account':   (_) => const CreateAccountScreen(isRestore: true),
        '/restore':           (_) => const CreateAccountScreen(isRestore: true),
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
      AppLockService.onForeground(context);
    } else if (s == AppLifecycleState.paused) {
      appState.onScreenOff();
      AppLockService.onBackground();
    } else if (s == AppLifecycleState.inactive ||
               s == AppLifecycleState.detached) {
      appState.onScreenOff();
    }
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
