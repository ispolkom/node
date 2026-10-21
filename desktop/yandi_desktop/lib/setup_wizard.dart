import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';

import 'platform_files.dart';
import 'password_strength.dart';
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
  /// `admin` — отдельный пароль администратора (пусто: совпадает с паролем входа).
  final Future<void> Function(String password, String phrase, String admin) finish;

  /// Восстановление из файла копии: путь, сид-фраза, новый пароль этой установки.
  final Future<void> Function(String path, String phrase, String password) restore;

  /// Печать листа с сид-фразой прямо из памяти (по умолчанию команда `lp`, файл не создаётся); в тестах подменяется.
  final Future<bool> Function(String text)? printer;

  /// Только для тестов: детерминированная перестановка и выбор слов для проверки.
  final Random? random;
  const SetupWizard({
    super.key,
    required this.api,
    required this.preferences,
    required this.busy,
    required this.finish,
    required this.restore,
    this.printer,
    this.random,
  });
  @override
  State<SetupWizard> createState() => _SetupWizardState();
}

/// Слова и контрольное слово одной строкой для узла (внутренний разделитель U+001F; слово после него; пусто — без слова).
String joinSeedPhrase(String words, String word) => word.trim().isEmpty ? words : '$words\u001f${word.trim()}';

enum _Step { show, order, type, word, password, restore }

class _SetupWizardState extends State<SetupWizard> {
  /// Минимум длины пароля задаёт узел (8; 6 только на тестовом узле): мастер берёт его из ответа `/api/auth/seed/new`, до ответа — 8.
  int _minimum = 8;
  late final Random _random = widget.random ?? Random.secure();
  _Step step = _Step.show;
  int count = 24;
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
  // контрольное слово (необязательно): одно слово, которое админ придумывает и помнит сам
  final controlWord = TextEditingController(), controlRepeat = TextEditingController();
  String _word = '';
  final adminPassword = TextEditingController(), adminRepeat = TextEditingController();
  // восстановление из копии
  final path = TextEditingController(),
      phrase = TextEditingController(),
      newPassword = TextEditingController(),
      newRepeat = TextEditingController(),
      restoreWord = TextEditingController();
  String? backupInfo;
  // восстановление: есть ли файл связей, номер узла из фразы и подтверждение, что он совпал с записью
  bool hasFile = false, idMatches = false;
  String? previewId;
  /// Ровно та строка (слова и слово), для которой показан номер: подтверждение действует только для неё.
  String? previewFor;
  /// Файл копии, который показан и будет восстановлен (если указана папка — выбранный в ней).
  String? resolvedFile;

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
      final min = r['min_password'];
      setState(() {
        if (min is int && min >= 6 && min <= 64) _minimum = min;
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
    controlWord.clear();
    controlRepeat.clear();
    _word = '';
  }

  @override
  void dispose() {
    words.fillRange(0, words.length, '');
    for (final c in [...typed, password, repeat, adminPassword, adminRepeat, path, phrase, newPassword, newRepeat, controlWord, controlRepeat, restoreWord]) {
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
      step = _Step.word;
    });
  }

  /// Слово, которое легко угадать (предупреждение, не запрет).
  static bool weakWord(String w) {
    final lower = w.toLowerCase();
    const common = {
      'password', 'passw0rd', 'qwertyuiop', 'qwerty123', '12345678', '123456789', '1234567890', 'iloveyou', 'admin1234', 'letmein12', 'welcome1',
      'abc12345', '1q2w3e4r', '1qaz2wsx', 'qazwsxedc', 'football', 'baseball', 'sunshine', 'princess', 'trustno1', 'пароль12', 'пароль123',
      'парольпароль', 'йцукенгшщз', 'йцукен123', 'привет123', 'секретное', 'контрольное', 'контрольноеслово', 'controlword',
    };
    return common.contains(lower) || lower.runes.toSet().length <= 3 || RegExp(r'^[0-9]+$').hasMatch(lower);
  }

  /// Шаг контрольного слова: пусто — без слова; иначе одно слово 8–64 знака, дважды.
  void acceptWord({required bool skip}) {
    if (skip || controlWord.text.trim().isEmpty) {
      setState(() {
        _word = '';
        controlWord.clear();
        controlRepeat.clear();
        error = null;
        step = _Step.password;
      });
      return;
    }
    final w = controlWord.text.trim();
    if (RegExp(r'\s').hasMatch(w)) {
      setState(() => error = 'Контрольное слово — одно слово, без пробелов');
      return;
    }
    if (w.runes.length < 8) {
      setState(() => error = 'Контрольное слово не короче 8 знаков');
      return;
    }
    if (w.runes.length > 64) {
      setState(() => error = 'Контрольное слово не длиннее 64 знаков');
      return;
    }
    if (w != controlRepeat.text.trim()) {
      setState(() => error = 'Слова не совпали');
      return;
    }
    setState(() {
      _word = w;
      error = null;
      step = _Step.password;
    });
  }

  /// Слова и контрольное слово одной строкой для узла (внутренний разделитель U+001F; слово после него).
  static String joinPhrase(String words, String word) => joinSeedPhrase(words, word);

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
    final a = adminPassword.text;
    if (a.isNotEmpty) {
      if (a.runes.length < _minimum) {
        setState(() => error = 'Пароль администратора — не короче $_minimum символов');
        return;
      }
      if (a != adminRepeat.text) {
        setState(() => error = 'Пароли администратора не совпадают');
        return;
      }
      if (a == p) {
        setState(() => error = 'Пароль администратора должен отличаться от пароля пользователя');
        return;
      }
    }
    setState(() {
      sending = true;
      error = null;
    });
    final phrase = joinPhrase(words.join(' '), _word);
    password.clear();
    repeat.clear();
    adminPassword.clear();
    adminRepeat.clear();
    try {
      await widget.finish(p, phrase, a);
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  /// Лист для печати: ТОЛЬКО слова по номерам, больше ничего. Передаётся принтеру через стандартный ввод, на диск не пишется.
  String _sheet() {
    final b = StringBuffer();
    for (var i = 0; i < words.length; i++) {
      b.writeln('${(i + 1).toString().padLeft(2)}. ${words[i]}');
    }
    return b.toString();
  }

  static Future<bool> _lp(String text) async {
    try {
      final p = await Process.start('lp', ['-t', 'YANDI', '-o', 'job-sheets=none']);
      p.stdin.add(utf8.encode(text));
      await p.stdin.close();
      await p.stdout.drain<void>();
      await p.stderr.drain<void>();
      return await p.exitCode == 0;
    } catch (_) {
      return false;
    }
  }

  String? printNote;

  Future<void> printSheet() async {
    setState(() => printNote = null);
    final ok = await (widget.printer ?? _lp)(_sheet());
    if (!mounted) return;
    setState(() => printNote = ok
        ? 'Отправлено на печать. Система печати хранит задание временно: проверьте, что лист напечатан и очередь пуста.'
        : 'Печать недоступна: принтер или команда lp не найдены.');
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
      if (!mounted) return;
      // копия принадлежит узлу, который получился из фразы и слова? Иначе слово или фраза введены неверно
      final id = '${r['node_id'] ?? ''}';
      if (previewId != null && id.isNotEmpty && id != previewId) {
        setState(() {
          error = 'Копия принадлежит другому узлу: проверьте слова фразы и контрольное слово.';
          resolvedFile = null;
        });
        return;
      }
      final ms = (r['created_ms'] is num) ? (r['created_ms'] as num).toInt() : 0;
      final t = DateTime.fromMillisecondsSinceEpoch(ms, isUtc: true);
      String two(int n) => n.toString().padLeft(2, '0');
      final when = '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)} UTC';
      setState(() {
        resolvedFile = r['file'] is String ? r['file'] as String : path.text.trim();
        backupInfo = 'Копия №${r['counter']} от $when; узел $id';
      });
    } catch (e) {
      if (mounted) setState(() => error = e is ApiFailure ? e.message : 'Файл копии не открылся.');
    }
  }

  String get _restorePhrase => joinPhrase(phrase.text.trim().split(RegExp(r'\s+')).join(' '), restoreWord.text);

  /// Показать номер узла, который получится из фразы и слова, — до любой записи (опечатка не создаст молча чужой узел).
  Future<void> showId() async {
    final words = phrase.text.trim().split(RegExp(r'\s+'));
    if (words.length != 12 && words.length != 18 && words.length != 24) {
      setState(() => error = 'Введите 12, 18 или 24 слова сид-фразы');
      return;
    }
    final snapshot = _restorePhrase;
    setState(() {
      error = null;
      previewId = null;
      previewFor = null;
      idMatches = false;
    });
    try {
      final r = await widget.api.request('/api/auth/seed/preview', body: {'phrase': snapshot});
      // слово или фраза изменились, пока шёл запрос: номер относится к прежнему вводу, его не показываем и не подтверждаем
      if (!mounted || snapshot != _restorePhrase) return;
      setState(() {
        previewId = '${r['node_id']}';
        previewFor = snapshot;
      });
    } catch (e) {
      // отказ узла по фразе: слово не из списка, опечатка, слабое или составное контрольное слово
      if (mounted) setState(() => error = e is ApiFailure && e.status == 400 ? 'Фраза или контрольное слово не прошли проверку: проверьте слова (порядок, опечатки) и контрольное слово (одно слово, 8–64 знака).' : (e is ApiFailure ? e.message : 'Не удалось проверить фразу.'));
    }
  }

  static String groupId(String id) => [for (var i = 0; i < id.length; i += 4) id.substring(i, i + 4 > id.length ? id.length : i + 4)].join(' ');

  Future<void> runRestore() async {
    if (sending || widget.busy) return;
    final words = phrase.text.trim().split(RegExp(r'\s+'));
    if (words.length != 12 && words.length != 18 && words.length != 24) {
      setState(() => error = 'Введите 12, 18 или 24 слова сид-фразы');
      return;
    }
    if (previewId == null || !idMatches || previewFor != _restorePhrase) {
      setState(() => error = 'Сначала проверьте номер узла и подтвердите, что он совпадает с вашей записью');
      return;
    }
    if (hasFile && (path.text.trim().isEmpty || resolvedFile == null)) {
      setState(() => error = 'Выберите файл копии и откройте его (Enter в поле пути) или снимите отметку');
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
    final p = newPassword.text, f = _restorePhrase, file = resolvedFile ?? path.text.trim(), withFile = hasFile;
    phrase.clear();
    restoreWord.clear();
    newPassword.clear();
    newRepeat.clear();
    try {
      if (withFile) {
        await widget.restore(file, f, p);
      } else {
        // без файла: те же ключи из сида (личность, ключ подписи списка узлов); телефоны сопрягаются заново
        await widget.finish(p, f, '');
      }
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  List<Widget> _restore() => [
    const UiText(
      'Восстановление по сид-фразе. Нужны сид-фраза и, если оно было, контрольное слово. Файл связей (.yandibak) необязателен: из него вернутся сопряжённые телефоны, контакты и группы. Пароль прежней установки не переносится: задайте новый.',
    ),
    const SizedBox(height: 12),
    for (final (controller, label, hint) in [
      (phrase, 'Сид-фраза (12, 18 или 24 слова)', true),
      (restoreWord, 'Контрольное слово (если было)', true),
    ])
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: TextField(
          controller: controller,
          obscureText: hint,
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
          enabled: !sending && !widget.busy,
          onChanged: (_) => setState(() {
            previewId = null;
            previewFor = null;
            idMatches = false;
            resolvedFile = null;
            backupInfo = null;
          }),
          decoration: InputDecoration(labelText: tr(context, label), border: const OutlineInputBorder()),
        ),
      ),
    Wrap(
      children: [_button('Проверить номер узла', sending || widget.busy ? null : showId, filled: false)],
    ),
    if (previewId != null) ...[
      Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Text('${tr(context, 'Номер узла')}: ${groupId(previewId!)}', style: const TextStyle(fontWeight: FontWeight.bold, fontSize: 18)),
      ),
      CheckboxListTile(
        key: const ValueKey('id-matches'),
        value: idMatches,
        onChanged: (v) => setState(() => idMatches = v == true),
        controlAffinity: ListTileControlAffinity.leading,
        contentPadding: EdgeInsets.zero,
        title: const UiText('Номер совпадает с моей записью'),
      ),
    ],
    CheckboxListTile(
      key: const ValueKey('has-file'),
      value: hasFile,
      onChanged: (v) => setState(() {
        hasFile = v == true;
        if (!hasFile) {
          path.clear();
          backupInfo = null;
          resolvedFile = null;
        }
      }),
      controlAffinity: ListTileControlAffinity.leading,
      contentPadding: EdgeInsets.zero,
      title: const UiText('У меня есть файл связей (.yandibak) или папка yandi-node'),
    ),
    if (hasFile) ...[
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
          _button('Прикрепить файл', pick, filled: false),
        ],
      ),
      if (backupInfo != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(backupInfo!)),
    ],
    for (final (controller, label) in [
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
    PasswordHint(newPassword),
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
    child: OutlinedButton(
      onPressed: () {
        widget.preferences.english = !widget.preferences.english;
        widget.preferences.update();
      },
      style: OutlinedButton.styleFrom(
        foregroundColor: Icq.header,
        side: const BorderSide(color: Icq.header),
        shape: const RoundedRectangleBorder(),
        minimumSize: const Size(80, 32),
        textStyle: const TextStyle(fontSize: 13, fontFamily: 'YandiSans', fontFamilyFallback: ['NotoColorEmoji']),
      ),
      child: Text(widget.preferences.english ? 'Русский' : 'English'),
    ),
  );

  Widget _button(String label, VoidCallback? action, {bool filled = true}) => Padding(
    padding: const EdgeInsets.only(right: 12, top: 8),
    child: SolidButton(label, onPressed: action, tone: filled ? SolidTone.primary : SolidTone.light),
  );

  static const _cellWidth = 136.0, _cellHeight = 40.0;

  /// Слово в окне одного размера: длина слова размер окна не меняет, слишком длинное уменьшается.
  Widget _cell(Widget child, {bool filled = true}) => Container(
    width: double.infinity,
    height: _cellHeight,
    alignment: Alignment.center,
    padding: const EdgeInsets.symmetric(horizontal: 8),
    decoration: BoxDecoration(
      color: filled ? Icq.light : Icq.background,
      border: Icq.frame,
    ),
    child: FittedBox(fit: BoxFit.scaleDown, child: child),
  );

  /// Сетка из окон одного размера: по три слова в строке (12 слов — 4 строки, 24 слова — 8 строк).
  Widget _numbered(List<String> list) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      for (var r = 0; r * 3 < list.length; r++)
        Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Row(
            children: [
              for (var i = r * 3; i < r * 3 + 3; i++)
                Expanded(
                  child: Padding(
                    padding: EdgeInsets.only(right: i % 3 == 2 ? 0 : 8),
                    child: i >= list.length ? const SizedBox(height: _cellHeight) : _cell(
                    Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(
                            text: '${i + 1}. ',
                            style: const TextStyle(color: Icq.dark),
                          ),
                          TextSpan(
                            text: list[i],
                            style: const TextStyle(fontWeight: FontWeight.bold, fontSize: 16),
                          ),
                        ],
                      ),
                      maxLines: 1,
                    ),
                    ),
                  ),
                ),
            ],
          ),
        ),
    ],
  );

  List<Widget> _show() => [
    const UiText('Шаг 1 из 5'),
    const SizedBox(height: 8),
    const UiText(
      'Запишите слова на бумаге в том же порядке, в каком они показаны на экране. По ним (и по контрольному слову, если зададите) узел восстанавливается на новом компьютере: та же личность, то же доверие к узлу. Никому не показывайте эти слова и не фотографируйте экран.',
    ),
    const SizedBox(height: 12),
    Align(
      alignment: Alignment.centerLeft,
      child: FittedBox(
        fit: BoxFit.scaleDown,
        child: UiText('Выберите степень сложности сид-фразы', style: const TextStyle(fontWeight: FontWeight.bold)),
      ),
    ),
    const SizedBox(height: 4),
    Row(
      children: [
        Expanded(child: DropdownButton<int>(
          isExpanded: true,
          value: count,
          items: const [
            DropdownMenuItem(value: 12, child: UiText('12 слов (128 бит)')),
            DropdownMenuItem(value: 18, child: UiText('18 слов (192 бита)')),
            DropdownMenuItem(value: 24, child: UiText('24 слова (256 бит, рекомендуется)')),
          ],
          onChanged: loading
              ? null
              : (v) {
                  if (v == null || v == count) return;
                  count = v;
                  load();
                },
        )),
      ],
    ),
    const SizedBox(height: 12),
    if (loading) const UiText('Создаю сид-фразу…') else _numbered(words),
    if (printNote != null) Padding(padding: const EdgeInsets.only(top: 4), child: UiText(printNote!)),
    Wrap(
      children: [
        _button('Сохранил слова', loading || words.isEmpty ? null : toOrder),
        _button('Распечатать', loading || words.isEmpty ? null : printSheet, filled: false),
        _button(
          'Восстановить по сид-фразе',
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
    const UiText('Шаг 2 из 5'),
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
            SizedBox(
              width: _cellWidth,
              height: _cellHeight,
              child: ElevatedButton(
                onPressed: () => setState(() {
                  picked = [...picked, index];
                  error = null;
                }),
                style: ElevatedButton.styleFrom(
                  backgroundColor: Icq.panel,
                  foregroundColor: Icq.text,
                  elevation: 0,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  shape: const RoundedRectangleBorder(),
                  side: const BorderSide(color: Icq.dark),
                  textStyle: const TextStyle(fontSize: 16, fontFamily: 'YandiSans', fontFamilyFallback: ['NotoColorEmoji'], decoration: TextDecoration.none),
                ),
                child: FittedBox(fit: BoxFit.scaleDown, child: Text(words[index], style: const TextStyle(fontWeight: FontWeight.bold, fontSize: 16))),
              ),
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
    const UiText('Шаг 3 из 5'),
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

  List<Widget> _wordStep() => [
    const UiText('Шаг 4 из 5'),
    const SizedBox(height: 8),
    const UiText(
      'Контрольное слово (необязательно). Это ОДНО слово, которое вы придумаете и запомните сами. Без него сид-фраза не восстановит узел, даже если листок найдут. Храните слово отдельно от сид-фразы: забыли слово, значит узел потерян безвозвратно. Слово не печатается и нигде не сохраняется.',
    ),
    for (final (controller, label) in [
      (controlWord, 'Контрольное слово (не короче 8 знаков, регистр важен)'),
      (controlRepeat, 'Повтор контрольного слова'),
    ])
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: TextField(
          controller: controller,
          obscureText: true,
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
          onChanged: (_) => setState(() {}),
          decoration: InputDecoration(labelText: tr(context, label), border: const OutlineInputBorder()),
          onSubmitted: (_) => acceptWord(skip: false),
        ),
      ),
    if (controlWord.text.trim().isNotEmpty && weakWord(controlWord.text.trim()))
      const UiText('Слово слабое: его легко угадать. Лучше 10 и больше случайных знаков.'),
    Wrap(
      children: [
        _button('Далее', () => acceptWord(skip: false)),
        _button('Без контрольного слова', () => acceptWord(skip: true), filled: false),
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
    const UiText('Без слова любой, кто увидит листок с сид-фразой, получит ваш узел.'),
  ];

  List<Widget> _password() => [
    const UiText('Шаг 5 из 5'),
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
    PasswordHint(password),
    const SizedBox(height: 12),
    const UiText(
      'Пароль администратора (необязательно) открывает только админ-панель: жалобы, шлюзы, настройки узла. Он не открывает переписку. Если оставить пустым, админ-панель открывается паролем пользователя; назначить отдельный можно позже в «Настройках узла».',
    ),
    for (final (controller, label) in [
      (adminPassword, 'Пароль администратора'),
      (adminRepeat, 'Повтор пароля администратора'),
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
      _Step.word => _wordStep(),
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
