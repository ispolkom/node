import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
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
    final state = context.read<AppState>();
    await state.init();
    if (!mounted) return;
    if (state.isPaired) {
      Navigator.of(context).pushReplacementNamed('/home');
    } else {
      Navigator.of(context).pushReplacementNamed('/pair');
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
