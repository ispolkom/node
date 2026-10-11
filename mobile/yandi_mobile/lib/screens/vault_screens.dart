import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../crypto/vault_keys.dart';
import '../services/app_state.dart';
import '../services/vault_service.dart';
import '../theme.dart';
import '../widgets/secure_keyboard.dart';

/// Экраны закрытой среды: создание (секретные слова → повтор → пароль), вход по паролю, восстановление по словам.
/// На этих экранах окно закрыто от скриншотов и записи экрана, ввод — только нашей клавиатурой (системная не вызывается).

/// Окно закрыто, пока открыт хотя бы один такой экран (счётчик: переход с экрана на экран не снимает защиту на миг).
class _ScreenGuard {
  static const _ch = MethodChannel('com.yandi.yandi_mobile/files');
  static int _n = 0;
  static void enter() {
    if (_n++ == 0) _ch.invokeMethod('secure', {'on': true}).catchError((_) {});
  }

  static void leave() {
    if (--_n == 0) _ch.invokeMethod('secure', {'on': false}).catchError((_) {});
  }
}

/// После успешного входа: запустить приложение и открыть главный экран или сопряжение.
Future<void> enterVaultedApp(BuildContext context) async {
  final state = context.read<AppState>();
  final nav = Navigator.of(context);
  await state.init();
  nav.pushNamedAndRemoveUntil(state.isPaired ? '/home' : '/pair', (_) => false);
}

/// Общая часть: поля с точками, наша клавиатура снизу, переход между полями по «Ввод».
abstract class _SecretFormState<T extends StatefulWidget> extends State<T> {
  TextEditingController? _active;
  final Map<TextEditingController, FocusNode> _focus = {};
  final Set<TextEditingController> _shown = {};

  /// Поля по порядку (для «Ввод» — следующее; на последнем — [submit]).
  List<TextEditingController> get order;
  void submit();

  @override
  void initState() {
    super.initState();
    _ScreenGuard.enter();
  }

  @override
  void dispose() {
    for (final f in _focus.values) {
      f.dispose();
    }
    _ScreenGuard.leave();
    super.dispose();
  }

  Widget field(String label, TextEditingController c) {
    final f = _focus.putIfAbsent(c, () {
      final n = FocusNode();
      n.addListener(() {
        if (n.hasFocus && mounted) setState(() => _active = c);
      });
      return n;
    });
    final shown = _shown.contains(c);
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: TextField(
        controller: c,
        focusNode: f,
        obscureText: !shown,
        keyboardType: SecureKeyboard.inputType,
        autocorrect: false,
        enableSuggestions: false,
        enableIMEPersonalizedLearning: false,
        contextMenuBuilder: (_, __) => const SizedBox.shrink(),   // без «вставить / копировать»: секреты не ходят через буфер обмена
        style: const TextStyle(color: AppTheme.text, fontSize: 16),
        decoration: InputDecoration(
          labelText: label,
          filled: true,
          fillColor: AppTheme.surface,
          border: OutlineInputBorder(borderRadius: BorderRadius.circular(10), borderSide: BorderSide.none),
          suffixIcon: IconButton(
            icon: Icon(shown ? Icons.visibility_off : Icons.visibility, color: AppTheme.textSecondary),
            onPressed: () => setState(() => shown ? _shown.remove(c) : _shown.add(c)),
          ),
        ),
      ),
    );
  }

  void _enter() {
    final i = _active == null ? -1 : order.indexOf(_active!);
    if (i >= 0 && i + 1 < order.length) {
      _focus[order[i + 1]]?.requestFocus();
    } else {
      submit();
    }
  }

  Widget keyboard() {
    final c = _active;
    if (c == null) return const SizedBox.shrink();
    return SecureKeyboard(key: ValueKey(c), controller: c, onEnter: _enter);
  }

  Widget page({required String title, required List<Widget> children, bool canGoBack = false}) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        automaticallyImplyLeading: canGoBack,
        iconTheme: const IconThemeData(color: AppTheme.text),
        title: Text(title, style: const TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold)),
      ),
      body: SafeArea(
        child: Column(children: [
          Expanded(child: SingleChildScrollView(padding: const EdgeInsets.fromLTRB(20, 16, 20, 12), child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: children))),
          keyboard(),
        ]),
      ),
    );
  }

  Widget info(String text) => Padding(
        padding: const EdgeInsets.only(bottom: 16),
        child: Text(text, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 14, height: 1.35)),
      );

  Widget errorText(String? e) => e == null
      ? const SizedBox.shrink()
      : Padding(padding: const EdgeInsets.only(bottom: 12), child: Text(e, style: const TextStyle(color: Colors.redAccent, fontSize: 14)));

  Widget primary(String label, VoidCallback? onTap) => Padding(
        padding: const EdgeInsets.only(top: 4),
        child: FilledButton(
          style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black, padding: const EdgeInsets.symmetric(vertical: 14)),
          onPressed: onTap,
          child: Text(label, style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
        ),
      );

  Widget link(String label, VoidCallback onTap, {Color color = AppTheme.textSecondary}) =>
      TextButton(onPressed: onTap, child: Text(label, style: TextStyle(color: color)));

  Future<bool> confirm(String title, String text, String yes) async {
    final r = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(title, style: const TextStyle(color: AppTheme.text)),
        content: Text(text, style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Нет')),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(yes, style: const TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return r == true;
  }

  static String pwProblem(String a, String b) {
    if (a.runes.length < VaultKeys.minPasswordLength) return 'Пароль не короче ${VaultKeys.minPasswordLength} знаков';
    if (a != b) return 'Пароли не совпали';
    return '';
  }

  static List<TextEditingController> fourFields() => List.generate(VaultKeys.phraseFieldCount, (_) => TextEditingController());
  static List<String> texts(List<TextEditingController> l) => l.map((c) => c.text).toList();
  static void clear(Iterable<TextEditingController> l) {
    for (final c in l) {
      c.clear();
    }
  }
}

// ── Создание ─────────────────────────────────────────────────────────────────────────────────────────────────────────

class VaultSetupScreen extends StatefulWidget {
  const VaultSetupScreen({super.key});
  @override
  State<VaultSetupScreen> createState() => _VaultSetupScreenState();
}

class _VaultSetupScreenState extends _SecretFormState<VaultSetupScreen> {
  int _step = 0; // 0 — слова, 1 — повтор, 2 — пароль, 3 — создаём
  final _w1 = _SecretFormState.fourFields();
  final _w2 = _SecretFormState.fourFields();
  final _pw = TextEditingController();
  final _pw2 = TextEditingController();
  List<String>? _phrase; // принятые слова; null — без слов
  String? _error;

  @override
  List<TextEditingController> get order => switch (_step) { 0 => _w1, 1 => _w2, _ => [_pw, _pw2] };

  @override
  void submit() => switch (_step) { 0 => _next(), 1 => _save(), _ => _done() };

  @override
  void dispose() {
    for (final c in [..._w1, ..._w2, _pw, _pw2]) {
      c.dispose();
    }
    super.dispose();
  }

  void _goto(int step) => setState(() {
        _step = step;
        _error = null;
        _active = null;
      });

  void _next() {
    final words = _SecretFormState.texts(_w1);
    if (!VaultKeys.phraseStrongEnough(words)) {
      setState(() => _error = 'Нужно ${VaultKeys.phraseFieldCount} разных слова, в каждом не меньше ${VaultKeys.minPhraseFieldChars} знаков');
      return;
    }
    _goto(1);
  }

  Future<void> _skip() async {
    final ok = await confirm('Без секретных слов?', 'Если вы забудете пароль, восстановить данные будет невозможно: придётся удалить всё и начать заново. Пропустить?', 'Пропустить');
    if (!ok || !mounted) return;
    _phrase = null;
    _SecretFormState.clear(_w1);
    _goto(2);
  }

  void _save() {
    if (!VaultKeys.samePhrase(_SecretFormState.texts(_w1), _SecretFormState.texts(_w2))) {
      setState(() => _error = 'Слова не совпали. Проверьте и введите ещё раз, или вернитесь назад.');
      return;
    }
    _phrase = _SecretFormState.texts(_w1);
    _SecretFormState.clear(_w2);
    _goto(2);
  }

  Future<void> _done() async {
    final problem = _SecretFormState.pwProblem(_pw.text, _pw2.text);
    if (problem.isNotEmpty) {
      setState(() => _error = problem);
      return;
    }
    setState(() { _step = 3; _error = null; _active = null; });
    try {
      await VaultService.instance.create(_pw.text, _phrase);
    } catch (e) {
      if (!mounted) return;
      setState(() { _step = 2; _error = e is VaultPolicyError ? e.message : 'Не удалось создать хранилище: $e'; });
      return;
    }
    _SecretFormState.clear([..._w1, ..._w2, _pw, _pw2]);
    _phrase = null;
    if (!mounted) return;
    Navigator.of(context).pushReplacementNamed('/vault-unlock', arguments: true);
  }

  @override
  Widget build(BuildContext context) {
    switch (_step) {
      case 0:
        return page(title: 'Секретные слова', children: [
          info('Придумайте ${VaultKeys.phraseFieldCount} слова: каждое не короче ${VaultKeys.minPhraseFieldChars} знаков, все разные. По ним можно будет войти, если вы забудете пароль. '
              'Запишите их и храните отдельно от телефона: без пароля и без этих слов данные не вернуть. Слова никуда не отправляются.'),
          for (var i = 0; i < _w1.length; i++) field('Слово ${i + 1}', _w1[i]),
          errorText(_error),
          primary('Далее', _next),
          link('Пропустить (без восстановления)', _skip),
        ]);
      case 1:
        return page(title: 'Повторите слова', children: [
          info('Введите те же слова ещё раз, в тех же полях. Так мы проверим, что вы их записали верно.'),
          for (var i = 0; i < _w2.length; i++) field('Слово ${i + 1}', _w2[i]),
          errorText(_error),
          primary('Сохранить', _save),
          link('Назад', () => _goto(0)),
        ]);
      case 2:
        return page(title: 'Пароль для входа', children: [
          info('Короткий пароль, не меньше ${VaultKeys.minPasswordLength} знаков. Он нужен при каждом запуске приложения. Без него никто не прочтёт ваши сообщения и файлы.'),
          field('Пароль', _pw),
          field('Повторите пароль', _pw2),
          errorText(_error),
          primary('Готово', _done),
        ]);
      default:
        return Scaffold(
          backgroundColor: AppTheme.bg,
          body: const Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
            CircularProgressIndicator(color: AppTheme.accent),
            SizedBox(height: 16),
            Text('Создаём защищённое хранилище…', style: TextStyle(color: AppTheme.textSecondary)),
          ])),
        );
    }
  }
}

// ── Вход ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

class VaultUnlockScreen extends StatefulWidget {
  /// true — хранилище только что создано: пароль вводится ещё раз, как проверка.
  final bool justCreated;
  const VaultUnlockScreen({super.key, this.justCreated = false});
  @override
  State<VaultUnlockScreen> createState() => _VaultUnlockScreenState();
}

class _VaultUnlockScreenState extends _SecretFormState<VaultUnlockScreen> {
  final _pw = TextEditingController();
  bool _busy = false;
  bool _corrupt = false;
  String? _error;
  Timer? _tick;
  int _waitLeft = 0;

  @override
  List<TextEditingController> get order => [_pw];

  @override
  void dispose() {
    _tick?.cancel();
    _pw.dispose();
    super.dispose();
  }

  void _startWait(Duration d) {
    _tick?.cancel();
    setState(() => _waitLeft = d.inSeconds + 1);
    _tick = Timer.periodic(const Duration(seconds: 1), (t) {
      if (!mounted) return t.cancel();
      setState(() => _waitLeft = _waitLeft - 1);
      if (_waitLeft <= 0) t.cancel();
    });
  }

  @override
  Future<void> submit() async {
    if (_busy || _waitLeft > 0) return;
    if (_pw.text.isEmpty) {
      setState(() => _error = 'Введите пароль');
      return;
    }
    setState(() { _busy = true; _error = null; });
    try {
      await VaultService.instance.unlock(_pw.text);
      _pw.clear();
      if (!mounted) return;
      await enterVaultedApp(context);
      return;
    } on VaultWrongSecret catch (e) {
      _error = 'Неверный пароль (подряд неверных: ${e.failures})';
    } on VaultThrottled catch (e) {
      _startWait(e.wait);
      _error = 'Слишком много попыток';
    } on VaultCorrupt {
      _corrupt = true;
      _error = 'Хранилище ключей повреждено. Если резервной копии нет, остаётся удалить всё и начать заново.';
    } catch (e) {
      _error = 'Не удалось открыть: $e';
    }
    _pw.clear();
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    final waiting = _waitLeft > 0;
    return page(title: 'YANDI', children: [
      const SizedBox(height: 8),
      Center(child: Image.asset('assets/images/logo.png', width: 72)),
      const SizedBox(height: 16),
      info(widget.justCreated ? 'Хранилище создано. Введите пароль, чтобы войти.' : 'Введите пароль.'),
      field('Пароль', _pw),
      errorText(waiting ? '${_error ?? 'Подождите'}: ещё $_waitLeft с' : _error),
      primary(_busy ? 'Проверяем…' : 'Войти', (_busy || waiting) ? null : submit),
      if (!widget.justCreated) link('Забыл пароль', () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultRecoverScreen()))),
      if (_corrupt) info('Если резервной копии нет, данные не вернуть: приложение можно только удалить в настройках телефона и поставить заново.'),
    ]);
  }
}

// ── Восстановление ───────────────────────────────────────────────────────────────────────────────────────────────────

class VaultRecoverScreen extends StatefulWidget {
  const VaultRecoverScreen({super.key});
  @override
  State<VaultRecoverScreen> createState() => _VaultRecoverScreenState();
}

class _VaultRecoverScreenState extends _SecretFormState<VaultRecoverScreen> {
  final _w = _SecretFormState.fourFields();
  final _pw = TextEditingController();
  final _pw2 = TextEditingController();
  bool _busy = false;
  bool _noWords = false;
  String? _error;

  @override
  List<TextEditingController> get order => [..._w, _pw, _pw2];

  @override
  void dispose() {
    for (final c in [..._w, _pw, _pw2]) {
      c.dispose();
    }
    super.dispose();
  }

  @override
  Future<void> submit() async {
    if (_busy) return;
    final problem = _SecretFormState.pwProblem(_pw.text, _pw2.text);
    if (problem.isNotEmpty) {
      setState(() => _error = problem);
      return;
    }
    setState(() { _busy = true; _error = null; });
    try {
      await VaultService.instance.recoverWithPhrase(_SecretFormState.texts(_w), _pw.text);
      _SecretFormState.clear([..._w, _pw, _pw2]);
      if (!mounted) return;
      await enterVaultedApp(context);
      return;
    } on VaultNoRecovery {
      _noWords = true;
      _error = 'Секретные слова при создании не задавались: восстановить доступ нельзя. Остаётся удалить всё и начать заново.';
    } on VaultWrongSecret {
      _error = 'Слова не подошли. Слово 1 должно быть в поле 1, слово 2 в поле 2 и так далее; регистр и «ё/е» не важны.';
    } on VaultThrottled catch (e) {
      _error = 'Слишком много попыток, подождите ${e.wait.inSeconds} с';
    } on VaultPolicyError catch (e) {
      _error = e.message;
    } catch (e) {
      _error = 'Не удалось восстановить: $e';
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    return page(title: 'Забыл пароль', children: [
      info('Введите ваши секретные слова в тех же полях, что при создании, и задайте новый пароль.'),
      for (var i = 0; i < _w.length; i++) field('Слово ${i + 1}', _w[i]),
      field('Новый пароль', _pw),
      field('Повторите новый пароль', _pw2),
      errorText(_error),
      primary(_busy ? 'Проверяем…' : 'Восстановить', _busy ? null : submit),
      link('Назад', () => Navigator.of(context).pop()),
      if (_noWords) info('Без пароля и без слов данные не вернуть. Удалить приложение можно в настройках телефона, потом поставить заново.'),
    ]);
  }
}

// ── Из настроек: смена пароля и «Удалить всё» ────────────────────────────────────────────────────────────────────────

class VaultChangePasswordScreen extends StatefulWidget {
  const VaultChangePasswordScreen({super.key});
  @override
  State<VaultChangePasswordScreen> createState() => _VaultChangePasswordScreenState();
}

class _VaultChangePasswordScreenState extends _SecretFormState<VaultChangePasswordScreen> {
  final _old = TextEditingController();
  final _new = TextEditingController();
  final _new2 = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  List<TextEditingController> get order => [_old, _new, _new2];

  @override
  void dispose() {
    for (final c in [_old, _new, _new2]) {
      c.dispose();
    }
    super.dispose();
  }

  @override
  Future<void> submit() async {
    if (_busy) return;
    final problem = _SecretFormState.pwProblem(_new.text, _new2.text);
    if (problem.isNotEmpty) {
      setState(() => _error = problem);
      return;
    }
    setState(() { _busy = true; _error = null; });
    try {
      await VaultService.instance.changePassword(_old.text, _new.text);
      _SecretFormState.clear([_old, _new, _new2]);
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Пароль изменён')));
      Navigator.of(context).pop();
      return;
    } on VaultWrongSecret catch (e) {
      _error = 'Старый пароль неверный (подряд неверных: ${e.failures})';
    } on VaultThrottled catch (e) {
      _error = 'Слишком много попыток, подождите ${e.wait.inSeconds} с';
    } on VaultPolicyError catch (e) {
      _error = e.message;
    } catch (e) {
      _error = 'Не удалось сменить пароль: $e';
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    return page(title: 'Сменить пароль', canGoBack: true, children: [
      info('Данные не перешифровываются: меняется только пароль, которым они открываются.'),
      field('Старый пароль', _old),
      field('Новый пароль', _new),
      field('Повторите новый пароль', _new2),
      errorText(_error),
      primary(_busy ? 'Меняем…' : 'Сменить', _busy ? null : submit),
    ]);
  }
}

class VaultEraseScreen extends StatefulWidget {
  const VaultEraseScreen({super.key});
  @override
  State<VaultEraseScreen> createState() => _VaultEraseScreenState();
}

class _VaultEraseScreenState extends _SecretFormState<VaultEraseScreen> {
  static const _ch = MethodChannel('com.yandi.yandi_mobile/files');
  final _pw = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  List<TextEditingController> get order => [_pw];

  @override
  void dispose() {
    _pw.dispose();
    super.dispose();
  }

  @override
  void submit() => _erase(uninstall: false);

  Future<void> _erase({required bool uninstall}) async {
    if (_busy) return;
    if (_pw.text.isEmpty) {
      setState(() => _error = 'Введите пароль');
      return;
    }
    setState(() { _busy = true; _error = null; });
    try {
      await VaultService.instance.verifyPassword(_pw.text);
    } on VaultWrongSecret catch (e) {
      _fail('Неверный пароль (подряд неверных: ${e.failures})');
      return;
    } on VaultThrottled catch (e) {
      _fail('Слишком много попыток, подождите ${e.wait.inSeconds} с');
      return;
    } catch (e) {
      _fail('Не удалось проверить пароль: $e');
      return;
    }
    if (!mounted) return;
    final ok = await confirm('Стереть всё?', 'Все данные YANDI на этом телефоне будут стёрты без возможности восстановления: сообщения, файлы, ключи, настройки.', 'Стереть');
    if (!ok || !mounted) {
      setState(() => _busy = false);
      return;
    }
    final nav = Navigator.of(context);
    final messenger = ScaffoldMessenger.of(context);
    final left = await context.read<AppState>().eraseEverything();
    if (left != null) messenger.showSnackBar(SnackBar(content: Text('Стёрто не всё: $left')));
    if (uninstall) {
      try { await _ch.invokeMethod('uninstall'); } catch (_) {}
    }
    nav.pushNamedAndRemoveUntil('/vault-setup', (_) => false);
  }

  void _fail(String e) {
    _pw.clear();
    if (mounted) setState(() { _busy = false; _error = e; });
  }

  @override
  Widget build(BuildContext context) {
    return page(title: 'Удалить всё', canGoBack: true, children: [
      info('Будут стёрты все данные YANDI на этом телефоне: сообщения, файлы, ключи, настройки. Для подтверждения введите пароль. '
          'Вернуть данные потом нельзя: ни по паролю, ни по секретным словам.'),
      field('Пароль', _pw),
      errorText(_error),
      primary(_busy ? 'Проверяем…' : 'Стереть данные', _busy ? null : () => _erase(uninstall: false)),
      link('Стереть данные и удалить приложение', _busy ? () {} : () => _erase(uninstall: true), color: Colors.redAccent),
    ]);
  }
}
