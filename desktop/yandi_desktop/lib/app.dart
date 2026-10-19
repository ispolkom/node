import 'admin_panel.dart';
import 'embedded_player.dart' show PlaybackGuard;
import 'home_blocks.dart';
import 'theme.dart';
import 'dashboard.dart';
import 'preferences.dart';
import 'setup_wizard.dart';
import 'nick.dart';
import 'member_badge.dart';
import 'voice.dart' show wipeOpenedCopies;
import 'background_alerts.dart';
import 'dart:async';
import 'package:flutter/material.dart';
import 'domain/states.dart';
import 'transport/api.dart';
import 'calls.dart';
import 'features.dart';
import 'platform_files.dart';

class DesktopApp extends StatefulWidget {
  final Api api;
  const DesktopApp({super.key, required this.api});
  @override
  State<DesktopApp> createState() => _DesktopAppState();
}

class _DesktopAppState extends State<DesktopApp> with WidgetsBindingObserver {
  final preferences = AppPreferences();
  late final background = BackgroundAlerts(
    widget.api,
    preferences,
    () => mounted && entered && connected && !lockUnconfirmed && !busy,
  );
  final navigator = GlobalKey<NavigatorState>();
  AuthStage? auth;
  VaultState? vault;
  bool connected = false, entered = false, busy = false, visible = true;
  /// Мастер первой настройки: при уходе из окна скрывается (слова не остаются на экране), через [_wizardWipeAfter] пересоздаётся без введённого.
  int wizardGeneration = 0;
  Timer? wizardWipeTimer;
  static const Duration _wizardWipeAfter = Duration(minutes: 3);

  /// Сообщение «Доступно обновление …» из /api/update/status (null — нет).
  String? updateNotice;
  /// Ответ `/api/update/status` целиком (для закреплённого блока новостей на главной).
  Map<String, dynamic>? updateStatus;
  bool polling = false, lockUnconfirmed = false;
  int epoch = 0, tick = 0, formVersion = 0, operation = 0;
  String? error, notice;
  String page = 'Панель';
  /// Режим администратора (docs/ROLES.md): отдельное меню; сеанс держит узел 15 минут по паролю.
  bool adminMode = false;
  /// Счётчики колокольчика (только числа; считаются узлом без расшифровки, поэтому видны и при запертой среде).
  int alertMessages = 0, alertNotices = 0;
  /// Предупреждение после восстановления из копии (что не очищено); остаётся на экране, пока его не закроют.
  String? restoreWarning;
  List<Map<String, dynamic>> notices = [];
  int unreadNotices = 0;
  int alertCalls = 0;
  Map<String, dynamic>? status, speed;
  List<dynamic> nodes = [];
  Timer? timer;
  bool blurIgnored = false;
  final callsController = CallsController();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    // После закрытия диалога фокус возвращается не мгновенно: если окно так и
    // не стало активным (ушли в другое приложение), содержимое стираем.
    PlatformFiles.onClosed = () {
      final now = WidgetsBinding.instance.lifecycleState;
      if (now == null || now == AppLifecycleState.resumed) return;
      Timer(const Duration(milliseconds: 500), () {
        final st = WidgetsBinding.instance.lifecycleState;
        if (mounted &&
            !PlatformFiles.choosing &&
            st != null &&
            st != AppLifecycleState.resumed) {
          didChangeAppLifecycleState(st);
        }
      });
    };
    background;
    preferences.addListener(preferenceChanged);
    checkAuth();
    startTimer();
  }

  void preferenceChanged() {
    if (mounted) setState(() {});
  }

  void startTimer() {
    timer?.cancel();
    if (visible)
      timer = Timer.periodic(const Duration(seconds: 1), (_) => poll());
  }

  static const adminPages = {'Статус узла', 'Шлюзы', 'Жалобы', 'Настройки узла'};

  /// [keepAdmin]: потеря фокуса окна стирает содержимое, но не сеанс администратора — он живёт на узле ограниченное время и ничего личного
  /// не содержит; иначе после любого переключения окна страницы управления остались бы на экране без токена и отвечали бы 403.
  void wipe({bool keepAdmin = false}) {
    if (!keepAdmin) {
      // сеанс администратора не переживает блокировку, выход, обрыв связи и смену пользователя: токен забывается, админ-страницы закрываются
      AdminToken.value = null;
      adminMode = false;
      if (adminPages.contains(page)) page = 'Панель';
      notices = [];
      unreadNotices = 0;
    }
    preferences.reset();
    background.reset();
    epoch++;
    formVersion++;
    navigator.currentState?.popUntil((route) => route.isFirst);
    widget.api.cancel();
    status = speed = null;
    nodes = [];
    vault = null;
    nickAsked = false;
    MemberBadge.forget(); // визитки участников (картинки) в памяти не переживают запирание
    unawaited(
      wipeOpenedCopies(),
    ); // открытые копии вложений («Сохранить и открыть») затираются при запирании и выходе
  }

  void disconnect(ApiFailure failure) {
    wipe();
    connected = entered = false;
    error = failure.message;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Окно проигрывателя (встроенное или внешнее) забирает фокус: пока оно открыто, чат не стирается. Когда воспроизведение кончилось,
    // обычное правило действует снова (следующая потеря фокуса стирает содержимое).
    if (PlaybackGuard.active) {
      blurIgnored = state != AppLifecycleState.resumed;
      return;
    }
    // Системный диалог выбора файла забирает фокус: не стираем содержимое.
    if (PlatformFiles.choosing) {
      blurIgnored = state != AppLifecycleState.resumed;
      return;
    }
    // Возврат фокуса из диалога не должен стирать только что выбранный файл.
    if (blurIgnored && state == AppLifecycleState.resumed) {
      blurIgnored = false;
      return;
    }
    blurIgnored = false;
    visible = state == AppLifecycleState.resumed;
    wizardWipeTimer?.cancel();
    if (!visible && auth == AuthStage.setup) {
      wizardWipeTimer = Timer(_wizardWipeAfter, () {
        wizardGeneration++;
        if (mounted) setState(() {});
      });
    }
    timer?.cancel();
    wipe(keepAdmin: true);
    if (mounted) setState(() {});
    if (visible) {
      checkAuth(preserveEntry: entered);
      startTimer();
    }
  }

  @override
  void dispose() {
    timer?.cancel();
    wizardWipeTimer?.cancel();
    preferences.reset();
    background.dispose();
    preferences.removeListener(preferenceChanged);
    preferences.dispose();
    WidgetsBinding.instance.removeObserver(this);
    PlatformFiles.onClosed = null;
    widget.api.cancel();
    super.dispose();
  }

  Future<void> checkAuth({bool preserveEntry = false}) async {
    final task = ++operation;
    final own = ++epoch;
    if (mounted)
      setState(() {
        busy = true;
        error = null;
      });
    try {
      final next = parseAuth(await widget.api.request('/api/auth/state'));
      if (!mounted || own != epoch) return;
      setState(() {
        auth = next;
        connected = true;
        entered = preserveEntry && next == AuthStage.login;
      });
      if (entered && !lockUnconfirmed) await refreshVault(own);
    } catch (e) {
      if (mounted && own == epoch)
        setState(
          () => disconnect(e is ApiFailure ? e : const ApiFailure('protocol')),
        );
    } finally {
      if (mounted && task == operation)
        setState(() {
          busy = false;
        });
    }
  }

  Future<void> refreshVault(int own) async {
    final next = VaultState.fromJson(
      await widget.api.request('/api/vault/status'),
    );
    if (!mounted || own != epoch) return;
    if (vault != null &&
        ((!vault!.configured || vault!.unlocked) &&
                next.configured &&
                !next.unlocked ||
            !vault!.hasError && next.hasError)) {
      epoch++;
      preferences.reset();
      background.reset();
      widget.api.cancel();
      formVersion++;
      navigator.currentState?.popUntil((route) => route.isFirst);
      status = speed = null;
      nodes = [];
    }
    setState(() {
      vault = next;
      lockUnconfirmed = false;
    });
    if (entered && next.unlocked && !nickAsked) ensureNick();
  }

  bool nickAsked = false;

  /// Ник обязателен: пока профиль не заполнен, показывается окно, которое нельзя закрыть без ника.
  Future<void> ensureNick() async {
    nickAsked = true;
    if (!await nickMissing(widget.api) || !mounted) return;
    final ctx = navigator.currentContext;
    if (ctx == null || !ctx.mounted) {
      nickAsked = false;
      return;
    }
    await requireNick(ctx, widget.api);
  }

  Future<void> enter() async {
    if (busy) return;
    setState(() {
      entered = true;
    });
    await poll(force: true);
  }

  /// Колокольчик: сколько сообщений ждёт на узле и сколько непрочитанных извещений админ-панели. Содержимое не запрашивается.
  Future<void> refreshAlerts(int own) async {
    try {
      final r = await widget.api.request('/api/alerts');
      if (!mounted || own != epoch) return;
      int n(Object? v) => v is num ? v.toInt().clamp(0, 99999) : 0;
      final m = n(r['messages']), x = n(r['notices']), c = n(r['missed_calls']);
      if (m != alertMessages || x != alertNotices || c != alertCalls) setState(() { alertMessages = m; alertNotices = x; alertCalls = c; });
    } catch (_) {}
  }

  /// Число на колокольчике: ожидающие сообщения + непрочитанные чаты текущего сеанса + новые извещения админ-панели.
  int get alertTotal => alertMessages + background.unread.length + alertNotices + alertCalls;

  /// Подсказка к колокольчику: что именно посчитано (без имён и текста).
  String get alertHint {
    final parts = <String>[
      if (alertMessages > 0) '${preferences.english ? 'Messages waiting' : 'Сообщений ждёт'}: $alertMessages',
      if (background.unread.isNotEmpty) '${preferences.english ? 'Unread chats' : 'Непрочитанных чатов'}: ${background.unread.length}',
      if (alertCalls > 0) '${preferences.english ? 'Missed calls' : 'Пропущенных звонков'}: $alertCalls',
      if (alertNotices > 0) '${preferences.english ? 'Admin notices' : 'Извещений админ-панели'}: $alertNotices',
    ];
    final base = preferences.english ? 'OS notifications' : 'Уведомления ОС';
    if (parts.isEmpty) return base;
    return '$base\n${parts.join(' · ')}${preferences.english ? '\nContent is hidden while the vault is locked' : '\nСодержимое скрыто, пока закрытая среда заперта'}';
  }

  /// Извещения администратора и состояние сеанса админ-панели (ошибки не мешают: у старых узлов ручки нет).
  Future<void> refreshNotices(int own) async {
    try {
      final r = await widget.api.request('/api/notices');
      if (!mounted || own != epoch) return;
      final items = [
        for (final x in (r['items'] is List ? r['items'] as List : const []))
          if (x is Map) x.cast<String, dynamic>(),
      ];
      final active = r['admin_active'] == true;
      setState(() {
        notices = items;
        unreadNotices = (r['unread'] is num) ? (r['unread'] as num).toInt() : 0;
        if (adminMode && !active) {
          // сеанс администратора закончился: возвращаемся в личный аккаунт
          adminMode = false;
          AdminToken.value = null;
          page = 'Панель';
          notice = 'Сеанс администратора закончился. Войдите в админ-панель снова.';
          formVersion++;
        }
      });
    } catch (_) {}
  }

  /// Открыть админ-панель: если сеанса нет, спросить пароль администратора.
  Future<void> openAdmin() async {
    var ok = false;
    try {
      ok = (await widget.api.request('/api/admin/session'))['active'] == true;
    } catch (_) {}
    if (!ok) {
      final ctx = navigator.currentContext;
      if (ctx == null) return;
      ok = await showDialog<bool>(context: ctx, builder: (_) => AdminPasswordDialog(api: widget.api)) ?? false;
    }
    if (!ok || !mounted) return;
    ackNotices('Статус узла');
    setState(() {
      epoch++;
      preferences.reset();
      background.openPeer = null;
      widget.api.cancel();
      adminMode = true;
      page = 'Статус узла';
      notice = null;
      formVersion++;
    });
    poll(force: true);
  }

  /// Отметить прочитанным то, что показывает открытая страница (а не всё сразу при входе): новое событие того же типа останется новым.
  void ackNotices(String page) {
    const byPage = {
      'Жалобы': ['complaints', 'pair_requests'],
      'Статус узла': ['update'],
      'Настройки узла': ['proxies'],
    };
    final kinds = byPage[page];
    if (kinds == null) return;
    unawaited(widget.api.request('/api/notices/ack', body: {'kinds': kinds}).then((_) {}, onError: (_) {}));
  }

  /// Владелец открыл чат: пропущенные звонки просмотрены, счётчик на узле и на колоколе обнуляется.
  void ackCalls() {
    if (alertCalls == 0) return;
    setState(() => alertCalls = 0);
    unawaited(widget.api.request('/api/alerts/ack', method: 'POST', body: const {}).then((_) {}, onError: (_) {}));
  }

  /// Выйти из админ-панели: сеанс на узле закрывается.
  Future<void> leaveAdmin() async {
    try {
      await widget.api.request('/api/admin/session', method: 'DELETE');
    } catch (_) {}
    AdminToken.value = null;
    if (!mounted) return;
    setState(() {
      epoch++;
      preferences.reset();
      background.openPeer = null;
      widget.api.cancel();
      adminMode = false;
      page = 'Панель';
      notice = null;
      formVersion++;
    });
    poll(force: true);
  }

  Future<void> poll({bool force = false}) async {
    if (!visible ||
        !connected ||
        !entered ||
        busy ||
        polling ||
        lockUnconfirmed)
      return;
    polling = true;
    final own = epoch;
    final current = tick++;
    try {
      // Re-read startup flags, including after a daemon restart.
      if (force || current % 5 == 0) {
        final next = parseAuth(await widget.api.request('/api/auth/state'));
        if (!mounted || own != epoch) return;
        if (next != AuthStage.login) {
          setState(() {
            wipe();
            auth = next;
            entered = false;
          });
          return;
        }
      }
      await refreshVault(own);
      if (force || current % 3 == 0) {
        await refreshNotices(own);
        await refreshAlerts(own);
      }
      if (!mounted || own != epoch || page != 'Статус узла') return;
      if (force || current % 5 == 0) {
        final result = await widget.api.request('/api/status');
        if (!mounted || own != epoch) return;
        setState(() {
          status = result;
        });
      }
      if (force || current % 2 == 0) {
        final result = await widget.api.request('/api/speed');
        if (!mounted || own != epoch) return;
        setState(() {
          speed = result;
        });
      }
      if (force || current % 60 == 0) {
        // подписанное обновление, найденное узлом (сам узел ничего не ставит): только сообщение владельцу
        try {
          final u = await widget.api.request('/api/update/status');
          if (!mounted || own != epoch) return;
          final a = u['available'];
          setState(() {
            updateStatus = u;
            updateNotice = a is Map && a['version'] is String
                ? 'Доступно обновление YANDI ${a['version']}'
                : null;
          });
        } catch (_) {}
      }
      if (force || current % 10 == 0) {
        final result = await widget.api.request('/api/nodes');
        if (result['nodes'] is! List) throw const ApiFailure('protocol');
        if (!mounted || own != epoch) return;
        setState(() {
          nodes = result['nodes'] as List;
        });
      }
    } catch (e) {
      if (mounted && own == epoch)
        setState(
          () => disconnect(e is ApiFailure ? e : const ApiFailure('protocol')),
        );
    } finally {
      polling = false;
    }
  }

  Future<void> act(String path, Map<String, dynamic> body) async {
    if (busy || !visible) return;
    final task = ++operation;
    final isLock = path == '/api/vault/lock';
    final own = ++epoch;
    widget.api.cancel();
    setState(() {
      busy = true;
      error = notice = null;
      formVersion++;
      if (isLock) {
        status = speed = null;
        nodes = [];
        vault = null;
        navigator.currentState?.popUntil((route) => route.isFirst);
        lockUnconfirmed = true;
      }
    });
    try {
      final pending = widget.api.request(path, body: body);
      body.clear();
      final response = await pending;
      if (path == '/api/auth/restore' && response['warnings'] is List && (response['warnings'] as List).isNotEmpty) {
        // что не удалось очистить от секретов старого формата — владельцу, с точными путями
        restoreWarning = 'Копия восстановлена, но не всё очищено от секретов старого формата: ${(response['warnings'] as List).join('; ')}';
      }
      if (!mounted || own != epoch) return;
      if (path == '/api/auth/setup' || path == '/api/auth/restore') {
        setState(() {
          notice = path == '/api/auth/setup'
              ? 'Ключи созданы. Нода запускается…'
              : 'Копия восстановлена. Нода запускается…';
        });
        final until = DateTime.now().add(const Duration(seconds: 120));
        var ready = false;
        while (DateTime.now().isBefore(until) && mounted && own == epoch) {
          await Future<void>.delayed(const Duration(seconds: 1));
          try {
            final next = parseAuth(await widget.api.request('/api/auth/state'));
            if (!mounted || own != epoch) return;
            if (next != AuthStage.setup) {
              setState(() {
                auth = next;
                entered = false;
                notice = null;
              });
              ready = true;
              break;
            }
          } on ApiFailure {
            /* Only GET is retried during daemon handover. */
          }
        }
        if (!ready) throw const ApiFailure('timeout');
      } else if (path.startsWith('/api/auth/')) {
        final next = parseAuth(await widget.api.request('/api/auth/state'));
        if (!mounted || own != epoch) return;
        setState(() {
          auth = next;
          entered = false;
          notice = 'Доступ восстановлен.';
        });
      } else {
        await refreshVault(own);
        if (mounted && own == epoch)
          setState(() {
            notice = 'Действие выполнено.';
          });
      }
    } catch (e) {
      if (!mounted || own != epoch) return;
      final failure = e is ApiFailure ? e : const ApiFailure('protocol');
      if (path == '/api/auth/restore' && (failure.warnings.isNotEmpty || failure.filesWritten)) {
        // отказ после записи части файлов: владелец узнаёт, что файлы уже на диске (повтор может отказать из-за них), и пути из предупреждений (P19-3, P20-4)
        restoreWarning = '${failure.filesWritten ? 'Часть файлов копии записана; повтор восстановления может отказать из-за них. ' : ''}${failure.warnings.isEmpty ? '' : 'Не очищено, пропущено или не подтверждено: ${failure.warnings.join('; ')}'}';
      }
      setState(() {
        if (failure.kind == 'disconnected' || failure.kind == 'timeout') {
          disconnect(failure);
        } else {
          // отказ восстановления после записи файлов: мастер всё ещё на экране, поэтому предупреждение идёт и в текст ошибки
          error = path == '/api/auth/restore' && restoreWarning != null && (failure.warnings.isNotEmpty || failure.filesWritten) ? '${failure.message} $restoreWarning' : failure.message;
        }
        if (isLock) lockUnconfirmed = true;
      });
    } finally {
      body.clear();
      if (mounted && task == operation)
        setState(() {
          busy = false;
        });
    }
  }

  Widget button(String label, VoidCallback action) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 6),
    child: Align(
      alignment: Alignment.centerLeft,
      child: BevelButton(
        label: label,
        onPressed: busy ? null : action,
        primary: true,
        danger: label.startsWith('Остановить'),
      ),
    ),
  );
  Widget dashboard() => DashboardView(
    status: status,
    speed: speed,
    nodes: nodes,
    controls: Wrap(
      spacing: 12,
      runSpacing: 8,
      children: [
        SolidButton(
          'Запустить узел',
          onPressed: busy ? null : () => act('/api/node/start', {}),
        ),
        SolidButton(
          'Остановить узел',
          tone: SolidTone.danger,
          onPressed: busy ? null : () => act('/api/node/stop', {}),
        ),
      ],
    ),
  );

  /// Кнопки тёмно-зелёные внутри (страница настроек, формы закрытой среды).
  Widget solidScope(Widget child) => Builder(
    builder: (context) => Theme(data: Icq.solidButtons(Theme.of(context)), child: child),
  );

  Widget vaultPanel() => solidScope(vaultPanelBody());

  Widget vaultPanelBody() {
    final v = vault;
    if (v == null)
      return UiText(
        lockUnconfirmed
            ? 'Состояние блокировки не подтверждено'
            : 'Проверка закрытой среды…',
      );
    return PanelStack(
      children: [
        const UiText('Закрытая среда', style: TextStyle(fontSize: 22)),
        UiText(v.label),
        if (v.migrationPending)
          const UiText(
            'Перенос части старых данных не завершён и повторится при входе.',
          ),
        const UiText(
          'Пять неверных паролей подряд оставляют вход только по сид-фразе; двадцать неверных сид-фраз стирают закрытую среду на этом компьютере (личность восстанавливается по сид-фразе и файлу копии).',
        ),
        if (!v.configured)
          SecretForm(
            key: ValueKey('vault-setup-$formVersion'),
            title: 'Создать закрытую среду',
            labels: const [
              'Пароль владельца',
              'Повтор пароля',
              'Сид-фраза (12 или 24 слова)',
            ],
            busy: busy,
            submit: (values) async {
              if (values[0] != values[1])
                throw const FormatException('Пароли не совпадают');
              if (values[0].runes.length < 6)
                throw const FormatException('Пароль — не короче 6 символов');
              final phrase = values[2].trim();
              final n = phrase.split(RegExp(r'\s+')).length;
              if (n != 12 && n != 24)
                throw const FormatException(
                  'Введите 12 или 24 слова сид-фразы',
                );
              final payload = <String, dynamic>{
                'password': values[0],
                'phrase': phrase,
              };
              await act('/api/vault/setup', payload);
            },
          ),
        if (v.configured && !v.unlocked) ...[
          SecretForm(
            key: ValueKey('unlock-$formVersion'),
            title: 'Разблокировать',
            labels: const ['Пароль владельца'],
            busy: busy,
            submit: (values) =>
                act('/api/vault/unlock', {'password': values[0]}),
          ),
          SecretForm(
            key: ValueKey('recover-$formVersion'),
            title: 'Восстановить словами',
            labels: const [
              'Сид-фраза (12 или 24 слова)',
              'Новый пароль',
              'Повтор нового пароля',
            ],
            busy: busy,
            submit: (values) {
              validateNew(values[1], values[2], minimum: 6);
              return act('/api/vault/recover', {
                'phrase': values[0],
                'new_password': values[1],
              });
            },
          ),
        ],
        if (v.unlocked) ...[
          button('Запереть', () => act('/api/vault/lock', {})),
          SecretForm(
            key: ValueKey('change-$formVersion'),
            title: 'Сменить пароль',
            labels: const [
              'Старый пароль',
              'Новый пароль',
              'Повтор нового пароля',
            ],
            busy: busy,
            submit: (values) {
              validateNew(values[1], values[2], minimum: 6);
              return act('/api/vault/change_password', {
                'old_password': values[0],
                'new_password': values[1],
              });
            },
          ),
        ],
        UiText(
          v.autoLockSecs == 0
              ? 'Автоблокировка выключена.'
              : 'Автоблокировка: через ${(v.autoLockSecs / 60).ceil()} мин. Таймером управляет демон; опрос не считается активностью.',
        ),
        TimeoutForm(
          key: ValueKey('timeout-$formVersion'),
          busy: busy,
          initial: v.autoLockSecs,
          submit: (secs) => act('/api/vault/auto_lock', {'secs': secs}),
        ),
      ],
    );
  }

  /// Конец мастера: ключи из сид-фразы (пароль входа = пароль владельца), затем закрытая среда с теми же паролем и фразой.
  Future<void> createWithSeed(String password, String phrase, [String admin = '']) async {
    await act('/api/auth/setup', {
      'login_password': password,
      'login_password_repeat': password,
      if (admin.isNotEmpty) 'admin_password': admin,
      if (admin.isNotEmpty) 'admin_password_repeat': admin,
      'master_password': phrase,
      'master_password_repeat': phrase,
    });
    if (!mounted || error != null || auth == AuthStage.setup) return;
    setState(() => entered = true);
    await act('/api/vault/setup', {'password': password, 'phrase': phrase});
  }

  /// Восстановление на новой машине: файл копии + сид-фраза + новый пароль; затем закрытая среда открывается этим паролем.
  Future<void> restoreFromBackup(
    String file,
    String phrase,
    String password,
  ) async {
    await act('/api/auth/restore', {
      'path': file,
      'phrase': phrase,
      'password': password,
    });
    if (!mounted || error != null || auth == AuthStage.setup) return;
    setState(() => entered = true);
    await act('/api/vault/unlock', {'password': password});
  }

  static void validateNew(String a, String b, {int minimum = 8}) {
    if (a.runes.length < minimum)
      throw FormatException('Новый пароль — не короче $minimum символов');
    if (a != b) throw const FormatException('Пароли не совпадают');
  }

  Widget content() {
    if (!connected)
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const UiText('Нет связи с демоном', style: TextStyle(fontSize: 24)),
          const UiText(
            'Запустите YANDI под вашим пользователем. Оболочка подключается только к локальному Unix-сокету; интернет для этого не нужен.',
          ),
          if (lockUnconfirmed)
            const UiText('Состояние блокировки не подтверждено'),
          button('Повторить подключение', () => checkAuth()),
        ],
      );
    if (auth == AuthStage.setup)
      // в неактивном окне слова и введённое не рисуются (состояние хранится недолго, потом мастер создаётся заново)
      return Visibility(
        visible: visible,
        maintainState: true,
        child: SetupWizard(
          key: ValueKey('setup-wizard-$wizardGeneration'),
          api: widget.api,
          preferences: preferences,
          busy: busy,
          finish: createWithSeed,
          restore: restoreFromBackup,
        ),
      );
    if (!entered || auth == AuthStage.recovery)
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          UiText(
            auth == AuthStage.recovery
                ? 'Требуется восстановление'
                : 'Доступ к ноде',
            style: const TextStyle(fontSize: 24),
          ),
          if (auth == AuthStage.recovery)
            const UiText(
              'Проблема привязки ключей к устройству. Для перепривязки используйте yandi-keys recover в терминале; оболочка не выполняет перепривязку.',
            )
          else ...[
            const UiText(
              'Локальный вход разрешён демоном по uid владельца Unix-сокета. Пароль браузерного входа здесь не требуется. Закрытая среда открывается отдельным паролем.',
            ),
            button('Войти', enter),
          ],
          SecretForm(
            key: ValueKey('auth-recover-$formVersion'),
            title: 'Восстановить доступ',
            labels: const [
              'Сид-фраза, мастер-пароль или код восстановления',
              'Новый пароль входа',
              'Повтор нового пароля',
            ],
            busy: busy,
            submit: (v) {
              validateNew(v[1], v[2]);
              return act('/api/auth/recover', {
                'recovery_code': v[0],
                'new_login_password': v[1],
                'remember_me': false,
              });
            },
          ),
        ],
      );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (restoreWarning != null) NoticePanel(restoreWarning!, error: true),
        if (updateNotice != null) UiText(updateNotice!),
        if (background.unread.isNotEmpty)
          UiText('Новых чатов: ${background.unread.length}'),
        if (lockUnconfirmed && !adminMode) ...[
          const UiText('Состояние блокировки не подтверждено'),
          button('Повторить блокировку', () => act('/api/vault/lock', {})),
        ] else if (!adminMode &&
            vault != null &&
            !vault!.hasError &&
            (!vault!.configured || !vault!.unlocked))
          // первый запуск: среда создаётся сразу; после блокировки на любой странице — только вход
          // (адрес поддержки не секрет: его можно скопировать и пока среда заперта)
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              vaultPanel(),
              if (vault!.configured) ...[const SizedBox(height: 12), SupportBlock(signed: updateStatus?['support'])],
            ],
          )
        else if (page == 'Панель')
          // «Главная»: закреплённый блок новостей и обновлений, ниже «Поддержать проект», затем извещения администратора (если есть)
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              NewsBlock(update: updateStatus),
              const SizedBox(height: 12),
              SupportBlock(signed: updateStatus?['support']),
              const SizedBox(height: 12),
              NoticesCard(items: notices, onOpen: openAdmin),
            ],
          )
        else if (page == 'Статус узла')
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              NodeStatusView(status: status),
              const SizedBox(height: 12),
              dashboard(),
            ],
          )
        else if (page == 'Настройки')
          solidScope(Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          vaultPanel(),
          const Divider(),
          AnimatedBuilder(
            animation: preferences,
            builder: (context, _) => PanelStack(
              title: 'Настройки оболочки',
              children: [
                CheckboxListTile(
                  controlAffinity: ListTileControlAffinity.leading,
                  title: const UiText('Язык / Language'),
                  subtitle: UiText(preferences.english ? 'English' : 'Русский'),
                  value: preferences.english,
                  onChanged: (value) {
                    preferences.english = value!;
                    preferences.changed();
                  },
                ),
                const UiText(
                  'Язык применяется ко всем формам и состояниям оболочки. Пользовательский текст не переводится.',
                ),
                CheckboxListTile(
                  controlAffinity: ListTileControlAffinity.leading,
                  title: const UiText('Фоновые оповещения узловых чатов'),
                  value: preferences.background,
                  onChanged: (v) {
                    preferences.background = v!;
                    background.reset();
                    preferences.changed();
                  },
                ),
                CheckboxListTile(
                  controlAffinity: ListTileControlAffinity.leading,
                  title: const UiText('Системный звук'),
                  value: preferences.sound,
                  onChanged: (value) {
                    preferences.sound = value!;
                    preferences.changed();
                  },
                ),
                CheckboxListTile(
                  controlAffinity: ListTileControlAffinity.leading,
                  title: const UiText('Уведомления ОС'),
                  value: preferences.notifications,
                  onChanged: (value) {
                    preferences.notifications = value!;
                    preferences.changed();
                  },
                ),
                CheckboxListTile(
                  controlAffinity: ListTileControlAffinity.leading,
                  title: const UiText('Не беспокоить'),
                  value: preferences.dnd,
                  onChanged: (value) {
                    preferences.dnd = value!;
                    preferences.changed();
                  },
                ),
                const UiText(
                  'Оповещения не передают содержимое и имя в ОС. Фоновый опрос: до 32 доверенных узловых контактов и 8 групп. Для телефонов нужен отдельный API непрочитанных. Настройки действуют до закрытия оболочки.',
                ),
                if (preferences.failure != null) UiText(preferences.failure!),
              ],
            ),
          ),
          if (visible &&
              vault != null &&
              !vault!.hasError &&
              (!vault!.configured || vault!.unlocked))
            features(),
        ])) else if (adminMode && visible)
          // управление узлом не требует личной среды: жалобы, шлюзы и настройки узла открываются одним админ-сеансом (P14-9)
          (page == 'Настройки узла' ? solidScope(features()) : features())
        else if (!visible || vault == null)
          const UiText('Проверка закрытой среды…')
        else if (vault!.hasError || vault!.configured && !vault!.unlocked)
          vaultPanel()
        else if (page == 'Настройки узла')
          solidScope(features())
        else
          features(),
      ],
    );
  }

  Widget shellHeader() => Container(
    decoration: const BoxDecoration(
      gradient: LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: [Icq.headerLight, Icq.header],
      ),
      border: Border(bottom: BorderSide(color: Icq.light, width: 2)),
    ),
    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
    // содержимое шапки той же ширины и по тем же краям, что блоки в теле страницы
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: !entered && connected ? 500 : 1400,
        ),
        child: HeaderLayout(
          children: [
            Image.asset('assets/logo.png', width: 38, height: 38),
            const SizedBox(width: 16),
            Expanded(
              child: entered && connected
                  ? TabStrip(
                      header: true,
                      labels: adminMode
                          ? const ['Личный аккаунт', 'Статус узла', 'Шлюзы', 'Жалобы', 'Настройки узла']
                          : [
                              'Главная',
                              'Чат',
                              'Поиск',
                              'Группы',
                              'Настройки',
                              'О приложении',
                              unreadNotices > 0 ? 'Админ-панель ($unreadNotices)' : 'Админ-панель',
                            ],
                      selected: page == 'Панель' ? 'Главная' : page,
                      onSelected: busy
                          ? null
                          : (label) {
                              if (label.startsWith('Админ-панель')) {
                                openAdmin();
                                return;
                              }
                              if (label == 'Личный аккаунт') {
                                leaveAdmin();
                                return;
                              }
                              if (adminMode) ackNotices(label);
                              if (label == 'Чат') ackCalls();
                              setState(() {
                                epoch++;
                                preferences.reset();
                                background.openPeer = null;
                                widget.api.cancel();
                                page = label == 'Главная' ? 'Панель' : label;
                                notice = null;
                                formVersion++;
                              });
                              poll(force: true);
                            },
                    )
                  : const UiText(
                      'YANDI · локальная оболочка',
                      style: TextStyle(
                        color: Colors.white,
                        fontWeight: FontWeight.bold,
                      ),
                    ),
            ),
            const SizedBox(width: 8),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextButton(
                  style: TextButton.styleFrom(
                    foregroundColor: Icq.panel,
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                    textStyle: const TextStyle(
                      fontSize: 13,
                      fontFamily: 'YandiSans',
                      decoration: TextDecoration.none,
                    ),
                  ),
                  onPressed: () {
                    preferences.english = !preferences.english;
                    preferences.changed();
                  },
                  child: const Text('EN / RU'),
                ),
                IconButton(
                  color: Icq.panel,
                  padding: EdgeInsets.zero,
                  constraints: const BoxConstraints.tightFor(
                    width: 30,
                    height: 30,
                  ),
                  tooltip: preferences.english
                      ? 'System sound'
                      : 'Системный звук',
                  icon: Icon(
                    preferences.sound
                        ? Icons.volume_up_outlined
                        : Icons.volume_off_outlined,
                    size: 18,
                  ),
                  onPressed: () {
                    preferences.sound = !preferences.sound;
                    preferences.changed();
                  },
                ),
                Stack(
                  clipBehavior: Clip.none,
                  children: [
                    IconButton(
                      color: Icq.panel,
                      padding: EdgeInsets.zero,
                      constraints: const BoxConstraints.tightFor(
                        width: 30,
                        height: 30,
                      ),
                      tooltip: alertHint,
                      icon: Icon(
                        preferences.notifications
                            ? Icons.notifications_active_outlined
                            : Icons.notifications_none,
                        size: 18,
                      ),
                      onPressed: () {
                        preferences.notifications = !preferences.notifications;
                        preferences.changed();
                      },
                    ),
                    if (entered && connected && alertTotal > 0)
                      Positioned(
                        key: const ValueKey('alert-badge'),
                        right: -2,
                        top: -2,
                        child: IgnorePointer(
                          child: Container(
                            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 1),
                            constraints: const BoxConstraints(minWidth: 16, minHeight: 16),
                            decoration: BoxDecoration(color: const Color(0xffb3261e), borderRadius: BorderRadius.circular(8)),
                            child: Text(
                              alertTotal > 99 ? '99+' : '$alertTotal',
                              textAlign: TextAlign.center,
                              style: const TextStyle(color: Colors.white, fontSize: 10, fontWeight: FontWeight.bold),
                            ),
                          ),
                        ),
                      ),
                  ],
                ),
                if (entered)
                  IconButton(
                    color: Icq.panel,
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints.tightFor(
                      width: 30,
                      height: 30,
                    ),
                    tooltip: preferences.english
                        ? 'Log out (locks the vault)'
                        : 'Выйти (запирает закрытую среду)',
                    icon: const Icon(Icons.logout, size: 18),
                    onPressed: busy
                        ? null
                        : () async {
                            // выход ВСЕГДА запирает среду: кеш состояния оболочки мог устареть (среду открыл другой клиент), а блокировка идемпотентна
                            await act('/api/vault/lock', {});
                            if (!mounted || lockUnconfirmed) return;
                            setState(() {
                              wipe();
                              entered = false;
                              notice = error = null;
                            });
                          },
                  ),
              ],
            ),
          ],
        ),
      ),
    ),
  );

  bool Function() activeFor(int own) =>
      () =>
          visible &&
          connected &&
          entered &&
          own == epoch &&
          vault != null &&
          !vault!.hasError &&
          (!vault!.configured || vault!.unlocked);

  void lockedByCalls() {
    if (mounted) {
      setState(() {
        wipe();
      });
      poll(force: true);
    }
  }

  /// Calls live under the header on every page: incoming rings anywhere, the
  /// chat starts outgoing ones with the words «Позвонить» / «Видеозвонок».
  Widget callPanel() {
    final own = epoch;
    return CallsFeature(
      key: ValueKey('calls-$own-$formVersion'),
      embedded: true,
      controller: callsController,
      api: widget.api,
      isActive: activeFor(own),
      onLocked: lockedByCalls,
      onFailure: (failure) {
        if (mounted) setState(() => disconnect(failure));
      },
    );
  }

  Widget features() {
    final own = epoch;
    return ShellFeatures(
      key: ValueKey('$page-$formVersion'),
      api: widget.api,
      preferences: preferences,
      calls: callsController,
      onChatOpened: background.opened,
      onGroupOpened: (id) => background.opened('group:$id'),
      page: page,
      isActive: activeFor(own),
      onFailure: (failure) {
        if (mounted) setState(() => disconnect(failure));
      },
      onLocked: lockedByCalls,
    );
  }

  @override
  Widget build(BuildContext context) => PreferencesScope(
    preferences: preferences,
    child: MaterialApp(
      navigatorKey: navigator,
      title: 'YANDI',
      debugShowCheckedModeBanner: false,
      theme: Icq.theme,
      home: Scaffold(
        body: SafeArea(
          child: Column(
            children: [
              shellHeader(),
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.all(12),
                  child: Align(
                    alignment: Alignment.topCenter,
                    child: ConstrainedBox(
                      constraints: BoxConstraints(
                        maxWidth: !entered && connected ? 500 : 1400,
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          if (busy) const LinearProgressIndicator(),
                          if (error != null && connected)
                            NoticePanel(error!, error: true),
                          if (notice != null)
                            TransientNotice(
                              key: ValueKey(notice),
                              text: notice!,
                              onDone: () {
                                if (mounted) setState(() => notice = null);
                              },
                            ),
                          if (entered &&
                              connected &&
                              visible &&
                              vault != null &&
                              !vault!.hasError &&
                              (!vault!.configured || vault!.unlocked))
                            callPanel(),
                          if (!connected)
                            Panel(
                              title: 'Нет связи с демоном',
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  if (error != null) UiText(error!),
                                  const SizedBox(height: 8),
                                  const UiText(
                                    'Запустите YANDI под вашим пользователем. Оболочка подключается только к локальному Unix-сокету; интернет для этого не нужен.',
                                  ),
                                  if (lockUnconfirmed)
                                    const UiText(
                                      'Состояние блокировки не подтверждено',
                                    ),
                                  const SizedBox(height: 12),
                                  button(
                                    'Повторить подключение',
                                    () => checkAuth(),
                                  ),
                                ],
                              ),
                            )
                          else if (!entered ||
                              auth == AuthStage.setup ||
                              auth == AuthStage.recovery)
                            Padding(
                              padding: const EdgeInsets.only(top: 36),
                              child: Panel(title: 'YANDI', child: content()),
                            )
                          else
                            content(),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    ),
  );
}

class SecretForm extends StatefulWidget {
  final String title;
  final String? description;
  final List<String> labels;
  final bool busy;
  final Future<void> Function(List<String>) submit;
  const SecretForm({
    super.key,
    required this.title,
    this.description,
    required this.labels,
    required this.busy,
    required this.submit,
  });
  @override
  State<SecretForm> createState() => _SecretFormState();
}

class _SecretFormState extends State<SecretForm> {
  late final fields = List.generate(
    widget.labels.length,
    (_) => TextEditingController(),
  );
  bool sending = false;
  String? error;
  Future<void> send() async {
    if (sending || widget.busy) return;
    setState(() {
      sending = true;
      error = null;
    });
    final bytes = fields.map((f) => SecretBytes(f.text)).toList();
    for (final field in fields) {
      field.clear();
    }
    final values = bytes.map((b) => b.decode()).toList();
    try {
      final pending = widget.submit(values);
      values.fillRange(0, values.length, '');
      for (final secret in bytes) {
        secret.clear();
      }
      await pending;
    } on FormatException catch (e) {
      if (mounted)
        setState(() {
          error = e.message;
        });
    } finally {
      values.fillRange(0, values.length, '');
      for (final secret in bytes) {
        secret.clear();
      }
      if (mounted)
        setState(() {
          sending = false;
        });
    }
  }

  @override
  void dispose() {
    for (final field in fields) {
      field.clear();
      field.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 12),
    child: Panel(
      title: widget.title,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (widget.description != null) UiText(widget.description!),
          for (var i = 0; i < fields.length; i++)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: TextField(
                controller: fields[i],
                obscureText: true,
                enableSuggestions: false,
                autocorrect: false,
                autofillHints: const [],
                enableIMEPersonalizedLearning: false,
                enabled: !sending && !widget.busy,
                decoration: InputDecoration(
                  labelText: tr(context, widget.labels[i]),
                  border: const OutlineInputBorder(),
                ),
                onSubmitted: (_) => send(),
              ),
            ),
          if (error != null) NoticePanel(error!, error: true),
          FilledButton(
            onPressed: sending || widget.busy ? null : send,
            child: UiText(sending ? 'Подождите…' : widget.title),
          ),
        ],
      ),
    ),
  );
}

class TimeoutForm extends StatefulWidget {
  final bool busy;
  final int initial;
  final Future<void> Function(int) submit;
  const TimeoutForm({
    super.key,
    required this.busy,
    required this.initial,
    required this.submit,
  });
  @override
  State<TimeoutForm> createState() => _TimeoutFormState();
}

class _TimeoutFormState extends State<TimeoutForm> {
  late bool never = widget.initial == 0;
  late final field = TextEditingController(
    text: widget.initial == 0
        ? '15'
        : (widget.initial / 60).ceil().clamp(1, 100).toString(),
  );
  String? error;
  @override
  void dispose() {
    field.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      CheckboxListTile(
        controlAffinity: ListTileControlAffinity.leading,
        title: const UiText('Никогда не блокировать'),
        value: never,
        onChanged: widget.busy
            ? null
            : (v) => setState(() {
                never = v == true;
                error = null;
              }),
      ),
      TextField(
        controller: field,
        enabled: !never,
        keyboardType: TextInputType.number,
        decoration: InputDecoration(
          labelText: tr(context, 'Блокировать через, минут (1–100)'),
        ),
      ),
      if (error != null) UiText(error!),
      FilledButton(
        onPressed: widget.busy
            ? null
            : () {
                if (never) {
                  setState(() => error = null);
                  widget.submit(0);
                  return;
                }
                final minutes = int.tryParse(field.text.trim());
                if (minutes == null || minutes < 1 || minutes > 100) {
                  setState(() {
                    error = 'Введите число от 1 до 100';
                  });
                  return;
                }
                setState(() {
                  error = null;
                });
                widget.submit(minutes * 60);
              },
        child: const UiText('Сохранить автоблокировку'),
      ),
    ],
  );
}

/// Сообщение об успехе («Действие выполнено», «Доступ восстановлен»): показывается несколько секунд и исчезает само.
class TransientNotice extends StatefulWidget {
  final String text;
  final VoidCallback onDone;
  const TransientNotice({super.key, required this.text, required this.onDone});
  @override
  State<TransientNotice> createState() => _TransientNoticeState();
}

class _TransientNoticeState extends State<TransientNotice> {
  Timer? timer;
  @override
  void initState() {
    super.initState();
    timer = Timer(const Duration(seconds: 5), widget.onDone);
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => NoticePanel(widget.text);
}
