import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../services/storage_service.dart';
import '../theme.dart';

/// Экран ввода / установки PIN-пароля.
///
/// Режимы:
///   [PinMode.unlock] — разблокировать приложение (повторный запуск)
///   [PinMode.set]    — задать PIN впервые или после восстановления по фразе
enum PinMode { unlock, set }

class PinScreen extends StatefulWidget {
  final PinMode mode;

  /// Маршрут после успешного ввода / установки PIN.
  final String nextRoute;

  const PinScreen({
    super.key,
    required this.mode,
    required this.nextRoute,
  });

  @override
  State<PinScreen> createState() => _PinScreenState();
}

class _PinScreenState extends State<PinScreen> {
  static const _pinLen = 6;

  String _entered     = '';
  String _firstEntry  = ''; // при установке — для подтверждения
  bool   _confirming  = false;
  String? _error;
  bool   _shaking     = false;

  String get _title {
    if (widget.mode == PinMode.unlock) return 'Введи пароль';
    if (_confirming) return 'Повтори пароль';
    return 'Создай пароль';
  }

  String get _hint {
    if (widget.mode == PinMode.unlock) return '';
    if (_confirming) return 'Введи то же снова';
    return '$_pinLen цифр или символов';
  }

  void _onKey(String key) {
    if (_entered.length >= _pinLen) return;
    setState(() {
      _entered += key;
      _error    = null;
    });
    if (_entered.length == _pinLen) {
      Future.delayed(const Duration(milliseconds: 120), _submit);
    }
  }

  void _onBackspace() {
    if (_entered.isEmpty) return;
    setState(() => _entered = _entered.substring(0, _entered.length - 1));
  }

  Future<void> _submit() async {
    if (_entered.length < 4) {
      _shake('Минимум 4 символа');
      return;
    }

    if (widget.mode == PinMode.unlock) {
      final ok = await StorageService.verifyPin(_entered);
      if (!mounted) return;
      if (ok) {
        await _afterUnlock();
      } else {
        _shake('Неверный пароль');
      }
      return;
    }

    // Режим установки PIN
    if (!_confirming) {
      setState(() {
        _firstEntry = _entered;
        _entered    = '';
        _confirming = true;
      });
      return;
    }

    if (_entered != _firstEntry) {
      _shake('Пароли не совпадают');
      setState(() { _confirming = false; _firstEntry = ''; });
      return;
    }

    await StorageService.savePin(_entered);
    if (!mounted) return;
    await _afterUnlock();
  }

  Future<void> _afterUnlock() async {
    // При режиме unlock инициализируем AppState (identity + ноды + WS).
    // При режиме set AppState ещё не инициализирован — тоже запускаем.
    final state = context.read<AppState>();
    if (!state.isPaired || widget.mode == PinMode.set) {
      await state.init();
    }
    if (!mounted) return;
    Navigator.of(context).pushReplacementNamed(widget.nextRoute);
  }

  void _shake(String msg) {
    setState(() { _error = msg; _entered = ''; _shaking = true; });
    Future.delayed(const Duration(milliseconds: 500), () {
      if (mounted) setState(() => _shaking = false);
    });
  }

  void _forgotPin() {
    // Очищаем PIN и отправляем на восстановление по фразе
    StorageService.clearPin();
    Navigator.of(context).pushReplacementNamed('/restore-account');
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      body: SafeArea(
        child: Column(
          children: [
            const SizedBox(height: 60),

            // Логотип
            const Text('YANDI', style: TextStyle(
              color: AppTheme.accent,
              fontSize: 22,
              fontWeight: FontWeight.bold,
              letterSpacing: 4,
            )),

            const SizedBox(height: 40),

            // Заголовок
            Text(_title, style: const TextStyle(
              color: AppTheme.text,
              fontSize: 20,
              fontWeight: FontWeight.w600,
            )),

            if (_hint.isNotEmpty) ...[
              const SizedBox(height: 6),
              Text(_hint, style: const TextStyle(
                color: AppTheme.textSecondary, fontSize: 13)),
            ],

            const SizedBox(height: 32),

            // Точки индикатора
            _PinDots(
              entered: _entered.length,
              total:   _pinLen,
              shaking: _shaking,
              hasError: _error != null,
            ),

            const SizedBox(height: 16),

            // Ошибка
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

            // Numpad
            _NumPad(onKey: _onKey, onBackspace: _onBackspace),

            const SizedBox(height: 24),

            // Забыл пароль (только в режиме разблокировки)
            if (widget.mode == PinMode.unlock)
              TextButton(
                onPressed: _forgotPin,
                child: const Text('Забыл пароль',
                    style: TextStyle(color: AppTheme.textSecondary, fontSize: 14)),
              ),

            const SizedBox(height: 16),
          ],
        ),
      ),
    );
  }
}

// ── Индикатор точек ───────────────────────────────────────────────────────────

class _PinDots extends StatelessWidget {
  final int  entered;
  final int  total;
  final bool shaking;
  final bool hasError;
  const _PinDots({
    required this.entered,
    required this.total,
    required this.shaking,
    required this.hasError,
  });

  @override
  Widget build(BuildContext context) {
    final dotColor = hasError ? Colors.redAccent : AppTheme.accent;
    return AnimatedSlide(
      offset: shaking ? const Offset(0.03, 0) : Offset.zero,
      duration: const Duration(milliseconds: 80),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: List.generate(total, (i) {
          final filled = i < entered;
          return AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            margin: const EdgeInsets.symmetric(horizontal: 8),
            width:  14,
            height: 14,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: filled ? dotColor : Colors.transparent,
              border: Border.all(
                color: filled ? dotColor : AppTheme.textSecondary,
                width: 2,
              ),
            ),
          );
        }),
      ),
    );
  }
}

// ── Numpad ────────────────────────────────────────────────────────────────────

class _NumPad extends StatelessWidget {
  final void Function(String) onKey;
  final VoidCallback           onBackspace;
  const _NumPad({required this.onKey, required this.onBackspace});

  static const _rows = [
    ['1', '2', '3'],
    ['4', '5', '6'],
    ['7', '8', '9'],
    ['', '0', '⌫'],
  ];

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 40),
      child: Column(
        children: _rows.map((row) => Row(
          mainAxisAlignment: MainAxisAlignment.spaceEvenly,
          children: row.map((key) {
            if (key.isEmpty) return const SizedBox(width: 72, height: 72);
            return _NumKey(
              label:    key,
              onTap:    () => key == '⌫' ? onBackspace() : onKey(key),
              isDelete: key == '⌫',
            );
          }).toList(),
        )).toList(),
      ),
    );
  }
}

class _NumKey extends StatelessWidget {
  final String    label;
  final VoidCallback onTap;
  final bool      isDelete;
  const _NumKey({required this.label, required this.onTap, this.isDelete = false});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        width: 72, height: 72,
        margin: const EdgeInsets.symmetric(vertical: 6),
        decoration: BoxDecoration(
          shape:  BoxShape.circle,
          color:  isDelete ? Colors.transparent : AppTheme.surface,
        ),
        child: Center(
          child: isDelete
              ? const Icon(Icons.backspace_outlined,
                    color: AppTheme.textSecondary, size: 22)
              : Text(label, style: const TextStyle(
                    color: AppTheme.text,
                    fontSize: 24,
                    fontWeight: FontWeight.w400)),
        ),
      ),
    );
  }
}
