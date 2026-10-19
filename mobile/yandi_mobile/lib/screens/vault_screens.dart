import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../crypto/vault_keys.dart';
import '../services/app_state.dart';
import '../services/vault_service.dart';
import '../theme.dart';
import '../widgets/password_hint.dart';
import '../widgets/secure_keyboard.dart';
import 'support_screen.dart';
import '../l10n.dart';

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
  try {
    await state.init();
  } on StateError catch (e) {
    // например, ключи личности не читаются: приложение не создаёт новую личность и не открывается, а говорит об этом
    if (context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('${e.message}')));
    return;
  }
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
        actions: const [LangToggle(), SizedBox(width: 8)],
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
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Нет'))),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(yes, style: const TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return r == true;
  }

  static String pwProblem(String a, String b) {
    if (a.runes.length < VaultKeys.minPasswordLength) return trf('Пароль не короче {n} знаков', {'n': VaultKeys.minPasswordLength});
    if (a != b) return tr('Пароли не совпали');
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
      setState(() => _error = trf('Нужно {n} разных слова, в каждом не меньше {m} знаков', {'n': VaultKeys.phraseFieldCount, 'm': VaultKeys.minPhraseFieldChars}));
      return;
    }
    _goto(1);
  }

  Future<void> _skip() async {
    final ok = await confirm(tr('Без секретных слов?'), tr('Если вы забудете пароль, восстановить данные будет невозможно: придётся удалить всё и начать заново. Пропустить?'), tr('Пропустить'));
    if (!ok || !mounted) return;
    _phrase = null;
    _SecretFormState.clear(_w1);
    _goto(2);
  }

  void _save() {
    if (!VaultKeys.samePhrase(_SecretFormState.texts(_w1), _SecretFormState.texts(_w2))) {
      setState(() => _error = tr('Слова не совпали. Проверьте и введите ещё раз, или вернитесь назад.'));
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
      setState(() { _step = 2; _error = e is VaultPolicyError ? e.message : trf('Не удалось создать хранилище: {e}', {'e': e}); });
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
        return page(title: tr('Секретные слова'), children: [
          info(trf('Придумайте {n} слова: каждое не короче {m} знаков, все разные. По ним можно будет войти, если вы забудете пароль. Запишите их и храните отдельно от телефона: без пароля и без этих слов данные не вернуть. Слова никуда не отправляются.', {'n': VaultKeys.phraseFieldCount, 'm': VaultKeys.minPhraseFieldChars})),
          for (var i = 0; i < _w1.length; i++) field(trf('Слово {n}', {'n': i + 1}), _w1[i]),
          errorText(_error),
          primary(tr('Далее'), _next),
          link(tr('Пропустить (без восстановления)'), _skip),
        ]);
      case 1:
        return page(title: tr('Повторите слова'), children: [
          info(tr('Введите те же слова ещё раз, в тех же полях. Так мы проверим, что вы их записали верно.')),
          for (var i = 0; i < _w2.length; i++) field(trf('Слово {n}', {'n': i + 1}), _w2[i]),
          errorText(_error),
          primary(tr('Сохранить'), _save),
          link(tr('Назад'), () => _goto(0)),
        ]);
      case 2:
        return page(title: tr('Пароль для входа'), children: [
          info(trf('Короткий пароль, не меньше {n} знаков. Он нужен при каждом запуске приложения. Без него никто не прочтёт ваши сообщения и файлы.', {'n': VaultKeys.minPasswordLength})),
          field(tr('Пароль'), _pw),
          PasswordHint(_pw),
          field(tr('Повторите пароль'), _pw2),
          errorText(_error),
          primary(tr('Готово'), _done),
        ]);
      default:
        return Scaffold(
          backgroundColor: AppTheme.bg,
          body: Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
            CircularProgressIndicator(color: AppTheme.accent),
            SizedBox(height: 16),
            Text(tr('Создаём защищённое хранилище…'), style: TextStyle(color: AppTheme.textSecondary)),
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
      setState(() => _error = tr('Введите пароль'));
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
      _error = trf('Неверный пароль (подряд неверных: {n})', {'n': e.failures});
    } on VaultThrottled catch (e) {
      _startWait(e.wait);
      _error = tr('Слишком много попыток');
    } on VaultCorrupt {
      _corrupt = true;
      _error = tr('Хранилище ключей повреждено. Если резервной копии нет, остаётся удалить всё и начать заново.');
    } catch (e) {
      _error = trf('Не удалось открыть: {e}', {'e': e});
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
      info(widget.justCreated ? tr('Хранилище создано. Введите пароль, чтобы войти.') : tr('Введите пароль.')),
      field(tr('Пароль'), _pw),
      errorText(waiting ? trf('{err}: ещё {n} с', {'err': _error ?? tr('Подождите'), 'n': _waitLeft}) : _error),
      primary(_busy ? tr('Проверяем…') : tr('Войти'), (_busy || waiting) ? null : submit),
      if (!widget.justCreated) link(tr('Забыл пароль'), () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultRecoverScreen()))),
      link(tr('Поддержать проект'), () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => SupportScreen(wallets: context.read<AppState>().wallets)))), // адрес не секрет: доступен и без входа
      if (_corrupt) info(tr('Если резервной копии нет, данные не вернуть: приложение можно только удалить в настройках телефона и поставить заново.')),
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
      _error = tr('Секретные слова при создании не задавались: восстановить доступ нельзя. Остаётся удалить всё и начать заново.');
    } on VaultWrongSecret {
      _error = tr('Слова не подошли. Слово 1 должно быть в поле 1, слово 2 в поле 2 и так далее; регистр и «ё/е» не важны.');
    } on VaultThrottled catch (e) {
      _error = trf('Слишком много попыток, подождите {n} с', {'n': e.wait.inSeconds});
    } on VaultPolicyError catch (e) {
      _error = e.message;
    } catch (e) {
      _error = trf('Не удалось восстановить: {e}', {'e': e});
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    return page(title: tr('Забыл пароль'), children: [
      info(tr('Введите ваши секретные слова в тех же полях, что при создании, и задайте новый пароль.')),
      for (var i = 0; i < _w.length; i++) field(trf('Слово {n}', {'n': i + 1}), _w[i]),
      field(tr('Новый пароль'), _pw),
      field(tr('Повторите новый пароль'), _pw2),
      errorText(_error),
      primary(_busy ? tr('Проверяем…') : tr('Восстановить'), _busy ? null : submit),
      link(tr('Назад'), () => Navigator.of(context).pop()),
      if (_noWords) info(tr('Без пароля и без слов данные не вернуть. Удалить приложение можно в настройках телефона, потом поставить заново.')),
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
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Пароль изменён'))));
      Navigator.of(context).pop();
      return;
    } on VaultWrongSecret catch (e) {
      _error = trf('Старый пароль неверный (подряд неверных: {n})', {'n': e.failures});
    } on VaultThrottled catch (e) {
      _error = trf('Слишком много попыток, подождите {n} с', {'n': e.wait.inSeconds});
    } on VaultPolicyError catch (e) {
      _error = e.message;
    } catch (e) {
      _error = trf('Не удалось сменить пароль: {e}', {'e': e});
    }
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    return page(title: tr('Сменить пароль'), canGoBack: true, children: [
      info(tr('Данные не перешифровываются: меняется только пароль, которым они открываются.')),
      field(tr('Старый пароль'), _old),
      field(tr('Новый пароль'), _new),
      PasswordHint(_new),
      field(tr('Повторите новый пароль'), _new2),
      errorText(_error),
      primary(_busy ? tr('Меняем…') : tr('Сменить'), _busy ? null : submit),
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
      setState(() => _error = tr('Введите пароль'));
      return;
    }
    setState(() { _busy = true; _error = null; });
    try {
      await VaultService.instance.verifyPassword(_pw.text);
    } on VaultWrongSecret catch (e) {
      _fail(trf('Неверный пароль (подряд неверных: {n})', {'n': e.failures}));
      return;
    } on VaultThrottled catch (e) {
      _fail(trf('Слишком много попыток, подождите {n} с', {'n': e.wait.inSeconds}));
      return;
    } catch (e) {
      _fail(trf('Не удалось проверить пароль: {e}', {'e': e}));
      return;
    }
    if (!mounted) return;
    final ok = await confirm(tr('Стереть всё?'), tr('Все данные YANDI на этом телефоне будут стёрты без возможности восстановления: сообщения, файлы, ключи, настройки.'), tr('Стереть'));
    if (!ok || !mounted) {
      setState(() => _busy = false);
      return;
    }
    final nav = Navigator.of(context);
    final messenger = ScaffoldMessenger.of(context);
    final left = await context.read<AppState>().eraseEverything();
    if (left != null) messenger.showSnackBar(SnackBar(content: Text(trf('Стёрто не всё: {n}', {'n': left}))));
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
    return page(title: tr('Удалить всё'), canGoBack: true, children: [
      info(tr('Будут стёрты все данные YANDI на этом телефоне: сообщения, файлы, ключи, настройки. Для подтверждения введите пароль. '
          'Вернуть данные потом нельзя: ни по паролю, ни по секретным словам.')),
      field(tr('Пароль'), _pw),
      errorText(_error),
      primary(_busy ? tr('Проверяем…') : tr('Стереть данные'), _busy ? null : () => _erase(uninstall: false)),
      link(tr('Стереть данные и удалить приложение'), _busy ? () {} : () => _erase(uninstall: true), color: Colors.redAccent),
    ]);
  }
}


// ── Экстренное стирание ──────────────────────────────────────────────────────────────────────────────────────────────

/// Быстрое стирание всех данных YANDI на телефоне без пароля (когда нет времени или пароля под рукой): подтверждается только словом.
class VaultPanicScreen extends StatefulWidget {
  const VaultPanicScreen({super.key});
  static const word = 'СТЕРЕТЬ', wordEn = 'DELETE';
  /// Подтверждено ли стирание введённым словом: русское или английское (на клавиатуре может не быть кириллицы), регистр и пробелы по краям не важны.
  static bool confirmed(String typed) {
    final t = typed.trim().toUpperCase();
    return t == word || t == wordEn;
  }
  @override
  State<VaultPanicScreen> createState() => _VaultPanicScreenState();
}

class _VaultPanicScreenState extends State<VaultPanicScreen> {
  final _word = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _word.dispose();
    super.dispose();
  }

  Future<void> _erase() async {
    if (_busy) return;
    if (!VaultPanicScreen.confirmed(_word.text)) {
      setState(() => _error = trf('Для стирания введите слово {w} (по-английски {en})', {'w': VaultPanicScreen.word, 'en': VaultPanicScreen.wordEn}));
      return;
    }
    setState(() { _busy = true; _error = null; });
    final nav = Navigator.of(context);
    final messenger = ScaffoldMessenger.of(context);
    final left = await context.read<AppState>().eraseEverything();
    if (left != null) messenger.showSnackBar(SnackBar(content: Text(trf('Стёрто не всё: {n}', {'n': left}))));
    nav.pushNamedAndRemoveUntil('/vault-setup', (_) => false);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(backgroundColor: AppTheme.surface, iconTheme: const IconThemeData(color: AppTheme.text), title: Text(tr('Экстренное стирание'), style: TextStyle(color: AppTheme.text))),
      body: ListView(padding: const EdgeInsets.all(16), children: [
        Text(tr('Сразу, без пароля, будут стёрты все данные YANDI на этом телефоне: сообщения, файлы, ключи, настройки. Вернуть их нельзя ни по паролю, ни по секретным словам. '
            'Чтобы не нажать случайно, введите слово СТЕРЕТЬ (или DELETE, если нет русской клавиатуры).'), style: TextStyle(color: AppTheme.textSecondary)),
        const SizedBox(height: 16),
        TextField(
          controller: _word,
          autocorrect: false,
          enableSuggestions: false,
          style: const TextStyle(color: AppTheme.text),
          decoration: InputDecoration(hintText: tr('СТЕРЕТЬ'), border: OutlineInputBorder()),
          onSubmitted: (_) => _erase(),
        ),
        if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(_error!, style: const TextStyle(color: Colors.redAccent))),
        const SizedBox(height: 16),
        ElevatedButton(
          style: ElevatedButton.styleFrom(backgroundColor: Colors.redAccent, foregroundColor: Colors.white),
          onPressed: _busy ? null : _erase,
          child: Text(_busy ? tr('Стираем…') : tr('Стереть всё сейчас')),
        ),
      ]),
    );
  }
}
