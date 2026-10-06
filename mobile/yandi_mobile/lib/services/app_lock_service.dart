import 'dart:async';
import 'package:flutter/material.dart';
import '../services/storage_service.dart';

/// Автоблокировка приложения через 60 секунд после ухода в фон.
///
/// Используется в _LifecycleWrapper (main.dart).
/// При блокировке поверх текущего стека показывается PinScreen(unlock).
class AppLockService {
  static const _lockAfterSeconds = 60;

  static DateTime? _backgroundedAt;
  static bool      _locked = false;

  /// Вызвать когда приложение уходит в фон.
  static void onBackground() {
    _backgroundedAt = DateTime.now();
  }

  /// Вызвать когда приложение возвращается на передний план.
  /// Если прошло > [_lockAfterSeconds] — показать PIN-экран.
  static Future<void> onForeground(BuildContext context) async {
    final bg = _backgroundedAt;
    if (bg == null || _locked) return;

    final elapsed = DateTime.now().difference(bg).inSeconds;
    if (elapsed < _lockAfterSeconds) return;

    final hasPin = await StorageService.hasPinSet();
    if (!hasPin) return;

    _locked = true;
    if (!context.mounted) { _locked = false; return; }

    // Показываем PIN поверх текущего экрана.
    // После успешного ввода возвращаемся обратно (pop).
    await Navigator.of(context, rootNavigator: true).push(
      PageRouteBuilder(
        opaque: true,
        barrierDismissible: false,
        pageBuilder: (_, __, ___) => _LockOverlay(
          onUnlocked: () {
            _locked = false;
            _backgroundedAt = null;
            Navigator.of(context, rootNavigator: true).pop();
          },
        ),
      ),
    );
    _locked = false;
  }
}

/// Оверлей блокировки — полноэкранный PIN без возможности закрыть назад.
class _LockOverlay extends StatefulWidget {
  final VoidCallback onUnlocked;
  const _LockOverlay({required this.onUnlocked});

  @override
  State<_LockOverlay> createState() => _LockOverlayState();
}

class _LockOverlayState extends State<_LockOverlay> {
  static const _pinLen = 6;
  String  _entered = '';
  String? _error;
  bool    _shaking = false;

  void _onKey(String key) {
    if (_entered.length >= _pinLen) return;
    setState(() { _entered += key; _error = null; });
    if (_entered.length == _pinLen) {
      Future.delayed(const Duration(milliseconds: 120), _verify);
    }
  }

  void _onBackspace() {
    if (_entered.isEmpty) return;
    setState(() => _entered = _entered.substring(0, _entered.length - 1));
  }

  Future<void> _verify() async {
    final ok = await StorageService.verifyPin(_entered);
    if (!mounted) return;
    if (ok) {
      widget.onUnlocked();
    } else {
      setState(() { _error = 'Неверный пароль'; _entered = ''; _shaking = true; });
      Future.delayed(const Duration(milliseconds: 500), () {
        if (mounted) setState(() => _shaking = false);
      });
    }
  }

  Future<void> _forgotPin() async {
    await StorageService.clearPin();
    if (!mounted) return;
    // Сброс: переходим на восстановление по фразе
    Navigator.of(context, rootNavigator: true).pushNamedAndRemoveUntil(
      '/restore-account', (_) => false);
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,  // нельзя закрыть кнопкой «Назад»
      child: Scaffold(
        backgroundColor: const Color(0xFF0A0A0A),
        body: SafeArea(
          child: Column(
            children: [
              const SizedBox(height: 60),
              const Text('YANDI', style: TextStyle(
                color: Color(0xFF00E5FF),
                fontSize: 22,
                fontWeight: FontWeight.bold,
                letterSpacing: 4,
              )),
              const SizedBox(height: 8),
              const Text('Приложение заблокировано',
                  style: TextStyle(color: Color(0xFF666666), fontSize: 13)),
              const SizedBox(height: 40),
              _buildDots(),
              const SizedBox(height: 16),
              AnimatedSwitcher(
                duration: const Duration(milliseconds: 200),
                child: _error != null
                    ? Text(_error!,
                          key: ValueKey(_error),
                          style: const TextStyle(
                              color: Colors.redAccent, fontSize: 13))
                    : const SizedBox(height: 18, key: ValueKey('empty')),
              ),
              const Spacer(),
              _buildNumpad(),
              const SizedBox(height: 24),
              TextButton(
                onPressed: _forgotPin,
                child: const Text('Забыл пароль',
                    style: TextStyle(color: Color(0xFF666666), fontSize: 14)),
              ),
              const SizedBox(height: 16),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildDots() {
    return AnimatedSlide(
      offset: _shaking ? const Offset(0.03, 0) : Offset.zero,
      duration: const Duration(milliseconds: 80),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: List.generate(_pinLen, (i) {
          final filled = i < _entered.length;
          final color  = _error != null ? Colors.redAccent : const Color(0xFF00E5FF);
          return AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            margin: const EdgeInsets.symmetric(horizontal: 8),
            width: 14, height: 14,
            decoration: BoxDecoration(
              shape:  BoxShape.circle,
              color:  filled ? color : Colors.transparent,
              border: Border.all(color: filled ? color : const Color(0xFF666666), width: 2),
            ),
          );
        }),
      ),
    );
  }

  Widget _buildNumpad() {
    const rows = [
      ['1','2','3'],
      ['4','5','6'],
      ['7','8','9'],
      ['', '0','⌫'],
    ];
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 40),
      child: Column(
        children: rows.map((row) => Row(
          mainAxisAlignment: MainAxisAlignment.spaceEvenly,
          children: row.map((k) {
            if (k.isEmpty) return const SizedBox(width: 72, height: 72);
            return GestureDetector(
              onTap: () => k == '⌫' ? _onBackspace() : _onKey(k),
              child: Container(
                width: 72, height: 72,
                margin: const EdgeInsets.symmetric(vertical: 6),
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: k == '⌫' ? Colors.transparent : const Color(0xFF1A1A1A),
                ),
                child: Center(
                  child: k == '⌫'
                      ? const Icon(Icons.backspace_outlined,
                            color: Color(0xFF666666), size: 22)
                      : Text(k, style: const TextStyle(
                            color: Color(0xFFEEEEEE),
                            fontSize: 24,
                            fontWeight: FontWeight.w400)),
                ),
              ),
            );
          }).toList(),
        )).toList(),
      ),
    );
  }
}
