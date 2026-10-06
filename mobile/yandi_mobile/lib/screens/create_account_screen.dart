import 'package:flutter/material.dart';
import '../crypto/identity.dart';
import '../theme.dart';

/// Экран создания / восстановления аккаунта через позиционную фразу.
///
/// 20 пронумерованных полей — пользователь заполняет любые из них.
/// Безопасность: позиция + содержимое + Argon2id.
/// Фраза НИКОГДА не сохраняется — только деривированный seed.
class CreateAccountScreen extends StatefulWidget {
  /// true — режим восстановления (те же поля, та же надпись на кнопке)
  final bool isRestore;
  const CreateAccountScreen({super.key, this.isRestore = false});

  @override
  State<CreateAccountScreen> createState() => _CreateAccountScreenState();
}

class _CreateAccountScreenState extends State<CreateAccountScreen>
    with SingleTickerProviderStateMixin {
  static const _maxFields = Identity.fieldCount; // 20
  static const _defaultVisible = 10;

  final _controllers = List.generate(_maxFields, (_) => TextEditingController());
  final _scrollCtrl  = ScrollController();

  bool _showAll   = false;
  bool _loading   = false;
  String? _error;

  late TabController _tabCtrl;

  @override
  void initState() {
    super.initState();
    _tabCtrl = TabController(length: 2, vsync: this,
        initialIndex: widget.isRestore ? 1 : 0);
  }

  @override
  void dispose() {
    for (final c in _controllers) c.dispose();
    _scrollCtrl.dispose();
    _tabCtrl.dispose();
    super.dispose();
  }

  int get _visibleCount => _showAll ? _maxFields : _defaultVisible;

  List<String> get _fields =>
      _controllers.map((c) => c.text).toList();

  bool get _hasAnyInput =>
      _controllers.any((c) => c.text.trim().isNotEmpty);

  // ── Создать / восстановить ─────────────────────────────────────────────────

  Future<void> _submit() async {
    if (!_hasAnyInput) {
      setState(() => _error = 'Заполни хотя бы одно поле');
      return;
    }
    setState(() { _loading = true; _error = null; });

    try {
      // Argon2id деривация — занимает 1-3 секунды
      await Identity.fromPhrase(_fields);
      if (!mounted) return;
      Navigator.of(context).pushReplacementNamed('/pair');
    } catch (e) {
      setState(() { _error = e.toString(); _loading = false; });
    }
  }

  // ── UI ─────────────────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        automaticallyImplyLeading: false,
        title: const Text('YANDI', style: TextStyle(
          color: AppTheme.accent, fontWeight: FontWeight.bold, letterSpacing: 2)),
        bottom: TabBar(
          controller: _tabCtrl,
          indicatorColor: AppTheme.accent,
          labelColor: AppTheme.accent,
          unselectedLabelColor: AppTheme.textSecondary,
          tabs: const [
            Tab(text: 'Создать аккаунт'),
            Tab(text: 'Восстановить'),
          ],
        ),
      ),
      body: TabBarView(
        controller: _tabCtrl,
        children: [
          _buildForm(isRestore: false),
          _buildForm(isRestore: true),
        ],
      ),
    );
  }

  Widget _buildForm({required bool isRestore}) {
    return Column(
      children: [
        // Инструкция
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 16, 20, 8),
          child: Text(
            isRestore
                ? 'Введи слова в те же поля с теми же номерами что при создании аккаунта. Порядок и номера полей важны.'
                : 'Введи свои слова или символы в любые поля. Запомни — какие поля и что написал. Пустые поля засчитываются.',
            textAlign: TextAlign.center,
            style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13),
          ),
        ),

        // Предупреждение для создания
        if (!isRestore)
          Container(
            margin: const EdgeInsets.symmetric(horizontal: 20, vertical: 4),
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              color: Colors.orange.withOpacity(0.1),
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: Colors.orange.withOpacity(0.3)),
            ),
            child: const Row(
              children: [
                Icon(Icons.warning_amber, color: Colors.orange, size: 18),
                SizedBox(width: 8),
                Expanded(
                  child: Text(
                    'Нет кнопки "Забыл фразу". Восстановить аккаунт можно только введя те же слова в те же поля.',
                    style: TextStyle(color: Colors.orange, fontSize: 12),
                  ),
                ),
              ],
            ),
          ),

        // Поля ввода
        Expanded(
          child: ListView.builder(
            controller: _scrollCtrl,
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
            itemCount: _visibleCount + 1, // +1 для кнопки расширения
            itemBuilder: (ctx, i) {
              if (i < _visibleCount) {
                return _FieldTile(
                  number:     i + 1,
                  controller: _controllers[i],
                );
              }
              // Кнопка показать все / скрыть
              return Padding(
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: TextButton.icon(
                  onPressed: () => setState(() => _showAll = !_showAll),
                  icon: Icon(
                    _showAll ? Icons.expand_less : Icons.expand_more,
                    color: AppTheme.textSecondary,
                  ),
                  label: Text(
                    _showAll ? 'Скрыть поля 11–20' : 'Показать поля 11–20',
                    style: const TextStyle(color: AppTheme.textSecondary),
                  ),
                ),
              );
            },
          ),
        ),

        // Ошибка
        if (_error != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 4),
            child: Text(_error!,
                style: const TextStyle(color: Colors.redAccent, fontSize: 13)),
          ),

        // Кнопка
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 8, 20, 32),
          child: SizedBox(
            width: double.infinity,
            height: 52,
            child: ElevatedButton(
              style: ElevatedButton.styleFrom(
                backgroundColor: AppTheme.accent,
                foregroundColor: Colors.black,
                shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(12)),
              ),
              onPressed: _loading ? null : _submit,
              child: _loading
                  ? const Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        SizedBox(
                          width: 18, height: 18,
                          child: CircularProgressIndicator(
                              strokeWidth: 2, color: Colors.black),
                        ),
                        SizedBox(width: 12),
                        Text('Генерация ключей…',
                            style: TextStyle(fontSize: 15)),
                      ],
                    )
                  : Text(
                      isRestore ? 'Восстановить' : 'Создать аккаунт',
                      style: const TextStyle(
                          fontSize: 16, fontWeight: FontWeight.bold),
                    ),
            ),
          ),
        ),
      ],
    );
  }
}

// ── Одно поле ввода ───────────────────────────────────────────────────────────

class _FieldTile extends StatefulWidget {
  final int number;
  final TextEditingController controller;
  const _FieldTile({required this.number, required this.controller});

  @override
  State<_FieldTile> createState() => _FieldTileState();
}

class _FieldTileState extends State<_FieldTile> {
  bool _filled = false;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onChanged);
  }

  void _onChanged() {
    final nowFilled = widget.controller.text.isNotEmpty;
    if (nowFilled != _filled) setState(() => _filled = nowFilled);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        children: [
          // Номер поля
          SizedBox(
            width: 36,
            child: Text(
              '${widget.number}',
              textAlign: TextAlign.right,
              style: TextStyle(
                color: _filled ? AppTheme.accent : AppTheme.textSecondary,
                fontSize: 13,
                fontWeight: _filled ? FontWeight.bold : FontWeight.normal,
              ),
            ),
          ),
          const SizedBox(width: 10),
          // Поле ввода
          Expanded(
            child: TextField(
              controller: widget.controller,
              style: const TextStyle(color: AppTheme.text, fontSize: 15),
              obscureText: true,  // скрываем ввод — никто не подсмотрит
              enableSuggestions: false,
              autocorrect: false,
              decoration: InputDecoration(
                isDense: true,
                contentPadding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                filled: true,
                fillColor: _filled
                    ? AppTheme.accent.withOpacity(0.08)
                    : AppTheme.surface,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(8),
                  borderSide: BorderSide.none,
                ),
                focusedBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(8),
                  borderSide: const BorderSide(color: AppTheme.accent, width: 1),
                ),
                hintText: '—',
                hintStyle:
                    const TextStyle(color: AppTheme.textSecondary, fontSize: 13),
                suffixIcon: _filled
                    ? IconButton(
                        icon: const Icon(Icons.clear,
                            size: 16, color: AppTheme.textSecondary),
                        onPressed: () => widget.controller.clear(),
                      )
                    : null,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
