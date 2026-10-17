import 'dart:math';

import 'package:flutter/material.dart';

import 'platform_files.dart';
import 'preferences.dart';
import 'theme.dart';
import 'transport/api.dart';

/// Мастер первого запуска: сид-фраза → проверка записи → пароль.
///
/// Сид создаёт узел (`/api/auth/seed/new`), слова только английские из стандартного списка; язык переключателя меняет лишь язык
/// инструкций. Слова и пароль живут только в памяти этого окна: на диск ничего не пишется, пока не принят пароль, а закрытие окна
/// на любом шаге выбрасывает сид (при следующем запуске будет новый).
class SetupWizard extends StatefulWidget {
  final Api api;
  final AppPreferences preferences;
  final bool busy;

  /// Создаёт ключи и закрытую среду (пароль, фраза из слов через пробел).
  final Future<void> Function(String password, String phrase) finish;

  /// Восстановление из файла копии: путь, сид-фраза, новый пароль этой установки.
  final Future<void> Function(String path, String phrase, String password) restore;

  /// Только для тестов: детерминированная перестановка и выбор слов для проверки.
  final Random? random;
  const SetupWizard({
    super.key,
    required this.api,
    required this.preferences,
    required this.busy,
    required this.finish,
    required this.restore,
    this.random,
  });
  @override
  State<SetupWizard> createState() => _SetupWizardState();
}

enum _Step { show, order, type, password, restore }

class _SetupWizardState extends State<SetupWizard> {
  static const _minimum = 6;
  late final Random _random = widget.random ?? Random.secure();
  _Step step = _Step.show;
  int count = 12;
  List<String> words = [];
  bool loading = true, sending = false;
  String? error;

  // проверка кликами: перемешанные номера слов и уже выбранные
  List<int> shuffled = [];
  List<int> picked = [];
  // проверка вводом: номера (с нуля) трёх слов
  List<int> asked = [];
  final typed = List.generate(3, (_) => TextEditingController());
  final password = TextEditingController(), repeat = TextEditingController();
  // восстановление из копии
  final path = TextEditingController(),
      phrase = TextEditingController(),
      newPassword = TextEditingController(),
      newRepeat = TextEditingController();
  String? backupInfo;

  @override
  void initState() {
    super.initState();
    load();
  }

  Future<void> load() async {
    setState(() {
      loading = true;
      error = null;
    });
    try {
      final r = await widget.api.request(
        '/api/auth/seed/new',
        body: {'words': count},
      );
      final list = (r['words'] as List).map((w) => '$w').toList();
      if (list.length != count) throw const ApiFailure('protocol');
      if (!mounted) return;
      setState(() {
        _wipe();
        words = list;
        step = _Step.show;
        loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        error = e is ApiFailure ? e.message : 'Не удалось получить сид-фразу.';
        loading = false;
      });
    }
  }

  void _wipe() {
    words.fillRange(0, words.length, '');
    words = [];
    shuffled = [];
    picked = [];
    asked = [];
    for (final c in typed) {
      c.clear();
    }
  }

  @override
  void dispose() {
    words.fillRange(0, words.length, '');
    for (final c in [...typed, password, repeat, path, phrase, newPassword, newRepeat]) {
      c.clear();
      c.dispose();
    }
    super.dispose();
  }

  void toOrder() {
    final order = List<int>.generate(words.length, (i) => i)..shuffle(_random);
    setState(() {
      shuffled = order;
      picked = [];
      error = null;
      step = _Step.order;
    });
  }

  void checkOrder() {
    final ok =
        picked.length == words.length &&
        [for (var i = 0; i < picked.length; i++) words[picked[i]] == words[i]]
            .every((v) => v);
    if (!ok) {
      setState(() {
        error = 'Порядок не совпал. Вернитесь и сверьте слова с записью.';
        picked = [];
      });
      return;
    }
    final pool = List<int>.generate(words.length, (i) => i)..shuffle(_random);
    setState(() {
      asked = (pool.take(3).toList()..sort());
      for (final c in typed) {
        c.clear();
      }
      error = null;
      step = _Step.type;
    });
  }

  void checkTyped() {
    var ok = true;
    for (var i = 0; i < 3; i++) {
      if (typed[i].text.trim().toLowerCase() != words[asked[i]]) ok = false;
    }
    if (!ok) {
      setState(() {
        error = 'Эти слова не совпали с записью. Проверьте запись.';
      });
      return;
    }
    setState(() {
      error = null;
      step = _Step.password;
    });
  }

  Future<void> create() async {
    if (sending || widget.busy) return;
    final p = password.text;
    if (p.runes.length < _minimum) {
      setState(() => error = 'Пароль — не короче $_minimum символов');
      return;
    }
    if (p != repeat.text) {
      setState(() => error = 'Пароли не совпадают');
      return;
    }
    setState(() {
      sending = true;
      error = null;
    });
    final phrase = words.join(' ');
    password.clear();
    repeat.clear();
    try {
      await widget.finish(p, phrase);
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  Future<void> pick() async {
    final chosen = await PlatformFiles.choose();
    if (chosen == null || !mounted) return;
    path.text = chosen;
    await inspect();
  }

  Future<void> inspect() async {
    setState(() {
      backupInfo = null;
      error = null;
    });
    try {
      final r = await widget.api.request('/api/auth/restore/inspect', body: {'path': path.text.trim()});
      if (mounted) setState(() => backupInfo = '${r['text']}');
    } catch (e) {
      if (mounted) setState(() => error = e is ApiFailure ? e.message : 'Файл копии не открылся.');
    }
  }

  Future<void> runRestore() async {
    if (sending || widget.busy) return;
    final words = phrase.text.trim().split(RegExp(r'\s+'));
    if (path.text.trim().isEmpty) {
      setState(() => error = 'Выберите файл копии');
      return;
    }
    if (words.length != 12 && words.length != 24) {
      setState(() => error = 'Введите 12 или 24 слова сид-фразы');
      return;
    }
    if (newPassword.text.runes.length < _minimum) {
      setState(() => error = 'Пароль — не короче $_minimum символов');
      return;
    }
    if (newPassword.text != newRepeat.text) {
      setState(() => error = 'Пароли не совпадают');
      return;
    }
    setState(() {
      sending = true;
      error = null;
    });
    final p = newPassword.text, f = phrase.text.trim(), file = path.text.trim();
    phrase.clear();
    newPassword.clear();
    newRepeat.clear();
    try {
      await widget.restore(file, f, p);
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  List<Widget> _restore() => [
    const UiText(
      'Восстановление из копии. Нужны файл копии (.yandibak) и сид-фраза. Пароль прежней установки не переносится: задайте новый пароль для этого компьютера.',
    ),
    const SizedBox(height: 12),
    Row(
      children: [
        Expanded(
          child: TextField(
            controller: path,
            enableSuggestions: false,
            autocorrect: false,
            decoration: InputDecoration(
              labelText: tr(context, 'Путь к файлу копии'),
              border: const OutlineInputBorder(),
            ),
            onSubmitted: (_) => inspect(),
          ),
        ),
        _button('Выбрать файл', pick, filled: false),
      ],
    ),
    if (backupInfo != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(backupInfo!)),
    for (final (controller, label) in [
      (phrase, 'Сид-фраза (12 или 24 слова)'),
      (newPassword, 'Новый пароль'),
      (newRepeat, 'Повтор нового пароля'),
    ])
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: TextField(
          controller: controller,
          obscureText: true,
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
          enabled: !sending && !widget.busy,
          decoration: InputDecoration(
            labelText: tr(context, label),
            border: const OutlineInputBorder(),
          ),
          onSubmitted: (_) => runRestore(),
        ),
      ),
    Wrap(
      children: [
        _button(sending ? 'Подождите…' : 'Восстановить', sending || widget.busy ? null : runRestore),
        _button(
          'Назад',
          () => setState(() {
            error = null;
            step = _Step.show;
          }),
          filled: false,
        ),
      ],
    ),
  ];

  Widget _language() => Align(
    alignment: Alignment.centerRight,
    child: TextButton(
      onPressed: () {
        widget.preferences.english = !widget.preferences.english;
        widget.preferences.update();
      },
      child: Text(widget.preferences.english ? 'Русский' : 'English'),
    ),
  );

  Widget _button(String label, VoidCallback? action, {bool filled = true}) =>
      Padding(
        padding: const EdgeInsets.only(right: 12, top: 8),
        child: filled
            ? FilledButton(onPressed: action, child: UiText(label))
            : OutlinedButton(onPressed: action, child: UiText(label)),
      );

  Widget _numbered(List<String> list) => Wrap(
    spacing: 8,
    runSpacing: 8,
    children: [
      for (var i = 0; i < list.length; i++)
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(border: Icq.frame),
          child: SelectableText.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: '${i + 1}. ',
                  style: const TextStyle(color: Icq.dark),
                ),
                TextSpan(
                  text: list[i],
                  style: const TextStyle(fontWeight: FontWeight.bold),
                ),
              ],
            ),
            // запрет копирования слов в буфер обмена: только запись от руки
            contextMenuBuilder: (context, state) => const SizedBox.shrink(),
          ),
        ),
    ],
  );

  List<Widget> _show() => [
    const UiText('Шаг 1 из 4'),
    const SizedBox(height: 8),
    const UiText(
      'Запишите слова на бумаге в том же порядке, в каком они показаны на экране. Без них нельзя восстановить пароль и перенести узел на новый компьютер: контакты и связи будут потеряны. Никому не показывайте эти слова и не фотографируйте экран.',
    ),
    const SizedBox(height: 12),
    Row(
      children: [
        const Flexible(child: UiText('Выберите степень сложности сид-фразы')),
        const SizedBox(width: 12),
        DropdownButton<int>(
          value: count,
          items: const [
            DropdownMenuItem(value: 12, child: UiText('12 слов (обычная)')),
            DropdownMenuItem(value: 24, child: UiText('24 слова (повышенная)')),
          ],
          onChanged: loading
              ? null
              : (v) {
                  if (v == null || v == count) return;
                  count = v;
                  load();
                },
        ),
      ],
    ),
    const SizedBox(height: 12),
    if (loading) const UiText('Создаю сид-фразу…') else _numbered(words),
    Wrap(
      children: [
        _button('Сохранил слова', loading || words.isEmpty ? null : toOrder),
        _button(
          'Восстановить из копии',
          () => setState(() {
            error = null;
            step = _Step.restore;
          }),
          filled: false,
        ),
      ],
    ),
  ];

  List<Widget> _order() => [
    const UiText('Шаг 2 из 4'),
    const SizedBox(height: 8),
    const UiText(
      'Проверка записи. Нажимайте на слова в том порядке, в каком они были показаны.',
    ),
    const SizedBox(height: 12),
    _numbered([for (var i = 0; i < words.length; i++) i < picked.length ? words[picked[i]] : '…']),
    const SizedBox(height: 12),
    Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final index in shuffled)
          if (!picked.contains(index))
            ActionChip(
              label: Text(words[index]),
              onPressed: () => setState(() {
                picked = [...picked, index];
                error = null;
              }),
            ),
      ],
    ),
    Wrap(
      children: [
        _button(
          'Сбросить',
          picked.isEmpty ? null : () => setState(() => picked = []),
          filled: false,
        ),
        _button(
          'ОК',
          picked.length == words.length ? checkOrder : null,
        ),
        _button(
          'Показать слова снова',
          () => setState(() {
            error = null;
            step = _Step.show;
          }),
          filled: false,
        ),
      ],
    ),
  ];

  List<Widget> _type() => [
    const UiText('Шаг 3 из 4'),
    const SizedBox(height: 8),
    const UiText(
      'Введите слова по номерам из своей записи. Сейчас слова на экране скрыты.',
    ),
    for (var i = 0; i < 3; i++)
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: TextField(
          controller: typed[i],
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
          decoration: InputDecoration(
            labelText: tr(context, 'Слово №{0}').replaceAll('{0}', '${asked[i] + 1}'),
            border: const OutlineInputBorder(),
          ),
          onSubmitted: (_) => checkTyped(),
        ),
      ),
    Wrap(
      children: [
        _button('ОК', checkTyped),
        _button(
          'Показать слова снова',
          () => setState(() {
            error = null;
            step = _Step.show;
          }),
          filled: false,
        ),
      ],
    ),
  ];

  List<Widget> _password() => [
    const UiText('Шаг 4 из 4'),
    const SizedBox(height: 8),
    const UiText(
      'Пароль даёт вход в вашу переписку без сид-фразы. Если забудете пароль, восстановите его по записанной сид-фразе. Пять неверных паролей подряд закроют вход по паролю; двадцать неверных сид-фраз сотрут узел на этом компьютере.',
    ),
    for (final (controller, label) in [
      (password, 'Пароль'),
      (repeat, 'Повтор пароля'),
    ])
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: TextField(
          controller: controller,
          obscureText: true,
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
          enabled: !sending && !widget.busy,
          decoration: InputDecoration(
            labelText: tr(context, label),
            border: const OutlineInputBorder(),
          ),
          onSubmitted: (_) => create(),
        ),
      ),
    _button(
      sending ? 'Подождите…' : 'Создать узел',
      sending || widget.busy ? null : create,
    ),
  ];

  @override
  Widget build(BuildContext context) {
    final body = switch (step) {
      _Step.show => _show(),
      _Step.order => _order(),
      _Step.type => _type(),
      _Step.password => _password(),
      _Step.restore => _restore(),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 12),
      child: Panel(
        title: 'Первичная настройка узла',
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _language(),
            ...body,
            if (error != null) ...[
              const SizedBox(height: 12),
              NoticePanel(error!, error: true),
            ],
            if (error != null && step == _Step.show && words.isEmpty)
              _button('Повторить', load),
          ],
        ),
      ),
    );
  }
}
