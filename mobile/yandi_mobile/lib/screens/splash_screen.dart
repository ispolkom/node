import 'package:flutter/material.dart';
import '../services/storage_service.dart';
import '../services/vault_service.dart';
import 'vault_screens.dart';
import '../theme.dart';

class SplashScreen extends StatefulWidget {
  const SplashScreen({super.key});
  @override
  State<SplashScreen> createState() => _SplashScreenState();
}

class _SplashScreenState extends State<SplashScreen> {
  @override
  void initState() {
    super.initState();
    _init();
  }

  Future<void> _init() async {
    // Сначала закрытая среда: нет хранилища — создаём (слова, пароль); есть, но закрыто — вводим пароль. Приложение стартует только после входа.
    await StorageService.init();
    final vault = VaultService.instance;
    final nav = Navigator.of(context);
    if (!await vault.exists()) {
      nav.pushReplacementNamed('/vault-setup');
    } else if (!vault.unlocked) {
      nav.pushReplacementNamed('/vault-unlock');
    } else {
      await enterVaultedApp(context);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Image.asset('assets/images/logo.png', width: 96),
            const SizedBox(height: 24),
            const Text('YANDI', style: TextStyle(
              color: AppTheme.accent,
              fontSize: 28,
              fontWeight: FontWeight.bold,
              letterSpacing: 4,
            )),
            const SizedBox(height: 40),
            const CircularProgressIndicator(color: AppTheme.accent),
          ],
        ),
      ),
    );
  }
}
