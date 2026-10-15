import 'message_actions.dart';
import 'contact_avatar.dart';
import 'theme.dart';
import 'search_page.dart';
import 'complaints_page.dart';
import 'content_policy.dart';
import 'preferences.dart';
import 'dart:io';
import 'dart:typed_data';
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'domain/shell_repository.dart';
import 'domain/states.dart';
import 'transport/api.dart';
import 'groups.dart';
import 'network.dart';
import 'calls.dart';
import 'attachments.dart';
import 'domain/file_repository.dart';
import 'platform_files.dart';
import 'voice.dart';
import 'profile_panel.dart';
import 'contact_tunnel.dart';
import 'devices.dart';
import 'advanced_settings.dart';

/// This subtree is disposed on lock, hide, disconnect and navigation.
class ShellFeatures extends StatefulWidget {
  final Api api;
  final String page;
  final AppPreferences? preferences;
  final CallsController? calls;
  final void Function(String)? onChatOpened;
  final void Function(String)? onGroupOpened;
  final void Function(ApiFailure) onFailure;
  final VoidCallback onLocked;
  final bool Function() isActive;
  const ShellFeatures({
    super.key,
    required this.api,
    required this.page,
    this.preferences,
    this.calls,
    this.onChatOpened,
    this.onGroupOpened,
    required this.onFailure,
    required this.onLocked,
    required this.isActive,
  });
  @override
  State<ShellFeatures> createState() => _ShellFeaturesState();
}

class _ShellFeaturesState extends State<ShellFeatures> {
  late final repo = ShellRepository(widget.api);
  Map<String, dynamic>? replyTo;
  final anchors = <String, GlobalKey>{};
  final card = TextEditingController(), name = TextEditingController();
  final search = TextEditingController(), composer = TextEditingController();
  final drafts = <String, String>{};
  List<Map<String, dynamic>> contacts = [], messages = [], trusted = [];
  late final voice = VoiceRecording(voiceBackendFactory(), () => voicePathFactory());
  /// Голосовое, которое не удалось отправить: хранится только в памяти (открытого файла в tmp не остаётся), пока открыт экран;
  /// можно повторить отправку. Для получателя это новое сообщение.
  Uint8List? failedVoice;
  /// Кому предназначалась неудавшаяся запись: повтор доступен только в этом чате, чужому адресату она не уходит.
  String? failedVoicePeer;
  // «Файлы чата»: отдельный вид без сообщений, как в телефоне.
  bool filesView = false;
  int fileTab = 0;
  static const fileTabs = [
    'Все',
    'Картинки',
    'Файлы',
    'Аудио',
    'Видео',
    'Ссылки',
  ];
  // кому уже пересылали сообщение в этом сеансе: не больше 20 получателей на сообщение
  final forwardedTo = <String, Set<String>>{};
  Map<String, dynamic>? selected;
  String? ownId, ownCard, lastProfileName, error, notice;
  bool loading = true, refreshing = false, sending = false;
  int revision = 0;
  File? pickedFile;
  int transferred = 0, transferSize = 0;
  bool choosingFile = false;
  String? editingId;
  final picked = <String>{};
  bool refreshError = false;
  bool addingContact = false;
  Timer? timer;
  bool get active => mounted && widget.isActive();

  @override
  void initState() {
    super.initState();
    voice.onLimit = () => finishVoice(send: true);
    voice.addListener(() {
      if (mounted) setState(() {});
    });
    if (const [
      'Поиск',
      'Жалобы',
      'Группы',
      'Шлюзы',
      'О приложении',
    ].contains(widget.page))
      return;
    refresh();
    if (widget.page != 'Настройки') {
      timer = Timer.periodic(const Duration(seconds: 5), (_) => refresh());
    }
  }

  @override
  void dispose() {
    revision++;
    timer?.cancel();
    voice.dispose();
    failedVoice = null;
    PlatformFiles.cancel().catchError((Object _) {});
    pickedFile = null;
    drafts.clear();
    contacts = messages = trusted = [];
    selected = null;
    ownCard = lastProfileName = null;
    for (final field in [card, name, search, composer]) {
      field.clear();
      field.dispose();
    }
    super.dispose();
  }

  void failure(Object e) {
    if (!mounted) return;
    final f = e is ApiFailure ? e : const ApiFailure('protocol');
    if (f.kind == 'disconnected' ||
        f.kind == 'timeout' ||
        f.kind == 'protocol') {
      revision++;
      widget.onFailure(f);
    } else {
      setState(() {
        error = f.message;
      });
    }
  }

  String peer(Map<String, dynamic> contact) {
    final short = '${contact['short_id']}';
    for (final t in trusted) {
      final id = t['id'];
      if (id is String && id.startsWith(short)) return id;
    }
    return short;
  }

  Future<void> refresh() async {
    if (!active || refreshing || sending) return;
    refreshing = true;
    final rev = revision;
    final currentPeer = selected == null ? null : peer(selected!);
    try {
      if (widget.page == 'Настройки') {
        final profile = await repo.profile();
        if (!active || rev != revision) return;
        final nextName = '${profile['display_name'] ?? ''}';
        if (loading || name.text == lastProfileName) name.text = nextName;
        lastProfileName = nextName;
      } else {
        final list = await repo.contacts();
        if (!active || rev != revision) return;
        final cards = await repo.trusted();
        if (!active || rev != revision) return;
        final status = await widget.api.request('/api/status');
        if (!active || rev != revision) return;
        setState(() {
          contacts = list;
          if (selected != null) {
            final matches = list.where((c) => c['id'] == selected!['id']);
            if (matches.isEmpty) {
              drafts.remove(currentPeer);
              selected = null;
              messages = [];
              composer.clear();
            } else {
              selected = matches.first;
            }
          }
          trusted = ShellRepository.list(cards, 'peers');
          ownCard = cards['card'] as String?;
          ownId = status['node_id'] as String?;
        });
        if (currentPeer != null && selected != null) {
          final history = await repo.history(currentPeer);
          if (!active ||
              rev != revision ||
              selected == null ||
              peer(selected!) != currentPeer)
            return;
          setState(() {
            messages = history;
          });
          widget.preferences?.observe(currentPeer, ownId, history);
        }
      }
      if (active && rev == revision)
        setState(() {
          if (refreshError) error = null;
          refreshError = false;
        });
    } catch (e) {
      if (active && rev == revision) {
        // Stale messages must not look like a current snapshot.
        setState(() {
          messages = [];
          refreshError = true;
        });
        failure(e);
      }
    } finally {
      refreshing = false;
      if (active && rev == revision)
        setState(() {
          loading = false;
        });
    }
  }

  Future<void> mutate(
    Future<void> Function() action, {
    VoidCallback? success,
  }) async {
    if (!active || sending) return;
    final rev = revision;
    setState(() {
      sending = true;
      refreshError = false;
      error = notice = null;
    });
    try {
      final v = VaultState.fromJson(
        await widget.api.request('/api/vault/status'),
      );
      if (!active || rev != revision) return;
      if (v.configured && !v.unlocked || v.hasError) {
        revision++;
        widget.onLocked();
        return;
      }
      await action();
      if (!active || rev != revision) return;
      success?.call();
      setState(() {
        notice = 'Действие выполнено.';
      });
    } catch (e) {
      if (active && rev == revision) failure(e);
    } finally {
      if (mounted)
        setState(() {
          sending = false;
        });
    }
    if (active && rev == revision && error == null) await refresh();
  }

  Future<void> choose(Map<String, dynamic> contact) async {
    widget.onChatOpened?.call(peer(contact));
    if (!active || sending) return;
    if (selected != null) drafts[peer(selected!)] = composer.text;
    revision++;
    setState(() {
      pickedFile = null;
      replyTo = null;
      editingId = null;
      picked.clear();
      filesView = false;
      fileTab = 0;
      transferred = transferSize = 0;
      selected = contact;
      messages = [];
      composer.text = drafts[peer(contact)] ?? '';
      loading = true;
      error = notice = null;
    });
    // A previous request may still be completing. Its revision guards its result.
    while (refreshing && active) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    if (active) await refresh();
  }

  Future<void> send() async {
    final contact = selected;
    if (contact == null || composer.text.trim().isEmpty || sending) return;
    final target = peer(contact), text = composer.text, quote = replyTo;
    final editing = editingId;
    if (findLink(text) != null) {
      // ссылки в YANDI не передаются: убираем из поля и сообщаем, отправлять без ссылки человек решает сам
      setState(() {
        composer.text = removeLinks(text);
        error = 'Ссылки в YANDI не передаются: ссылка удалена из сообщения. Проверьте текст и отправьте снова.';
      });
      return;
    }
    await mutate(
      () => editing != null
          ? repo.edit(target, editing, text)
          : repo.send(target, text, replyTo: quote),
      success: () {
        composer.clear();
        drafts.remove(target);
        replyTo = null;
        editingId = null;
      },
    );
  }

  Future<void> pickFile() async {
    if (choosingFile || sending || selected == null) return;
    choosingFile = true;
    final rev = revision;
    try {
      final path = await PlatformFiles.choose();
      if (!active || rev != revision || path == null) return;
      final bad = await fileVerdict(path);
      if (bad != null) {
        if (active) setState(() => error = bad);
        return;
      }
      setState(() {
        pickedFile = File(path);
        transferred = transferSize = 0;
      });
    } catch (e) {
      if (active && rev == revision) failure(e);
    } finally {
      choosingFile = false;
    }
  }

  Future<void> startVoice() async {
    if (sending || selected == null) return;
    final why = await voice.start();
    if (why != null && mounted) setState(() => error = why);
  }

  /// Завершить запись: отправить как файл (имя «Голосовое_…», как в телефоне) или отбросить; временный файл затирается.
  Future<void> finishVoice({required bool send}) async {
    final (path, secs) = await voice.stop();
    if (path == null) return;
    final f = File(path);
    try {
      if (send && secs >= 1 && active && selected != null) {
        // запись держим в памяти до результата: отправка не удалась — её можно повторить, а файл в tmp всё равно затирается
        Uint8List? copy;
        try {
          copy = await f.readAsBytes();
        } catch (_) {}
        final target = peer(selected!);
        setState(() => pickedFile = f);
        await sendFile();
        if (mounted && pickedFile?.path == f.path && copy != null) {
          setState(() {
            failedVoice = copy;
            failedVoicePeer = target;
          });   // pickedFile не сброшен успехом — отправка не удалась
        } else if (mounted && failedVoice != null) {
          setState(() => failedVoice = null);
        }
      }
    } finally {
      if (mounted && pickedFile?.path == f.path) {
        setState(() => pickedFile = null);
      }
      await wipeVoice(f);
    }
  }

  /// Повторить отправку неудавшегося голосового: запись из памяти кладётся во временный файл только на время отправки.
  Future<void> retryVoice() async {
    final bytes = failedVoice;
    if (bytes == null || sending || selected == null || failedVoicePeer != peer(selected!)) return;
    final f = File(await voicePathFactory());
    try {
      await f.parent.create(recursive: true);
      await f.writeAsBytes(bytes, flush: true);
      setState(() => pickedFile = f);
      await sendFile();
      if (mounted && pickedFile?.path != f.path) setState(() => failedVoice = null);
    } finally {
      if (mounted && pickedFile?.path == f.path) setState(() => pickedFile = null);
      await wipeVoice(f);
    }
  }

  Future<void> sendFile() async {
    final file = pickedFile, contact = selected;
    if (file == null || contact == null || sending) return;
    final rev = revision, target = peer(contact);
    await mutate(
      () async {
        await FileRepository(widget.api).upload(
          target,
          file,
          phone: contact['is_phone'] == true,
          isActive: () => active && revision == rev,
          progress: (done, total) {
            if (active && revision == rev)
              setState(() {
                transferred = done;
                transferSize = total;
              });
          },
        );
      },
      success: () {
        pickedFile = null;
        transferred = transferSize = 0;
      },
    );
  }

  Future<void> remove(Map<String, dynamic> contact) async {
    final rev = revision;
    final answer = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: UiText('Удалить контакт «${contact['name']}»?'),
        content: const UiText(
          'Удаляется запись контакта. Доверие карточке и доступ телефона сохраняются.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const UiText('Отмена'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const UiText('Удалить'),
          ),
        ],
      ),
    );
    if (!active || rev != revision || answer != true) return;
    await mutate(
      () => repo.remove('${contact['id']}'),
      success: () {
        if (selected?['id'] == contact['id']) {
          selected = null;
          messages = [];
          composer.clear();
        }
        drafts.remove(peer(contact));
      },
    );
  }

  Future<void> exportContacts() async {
    final rev = revision;
    String? path;
    try {
      path = await PlatformFiles.choose(save: true, name: 'contacts.json');
    } catch (_) {
      if (active && revision == rev)
        setState(() {
          error = 'Не удалось открыть диалог сохранения.';
        });
      return;
    }
    if (!active || revision != rev || path == null) return;
    await mutate(
      () => FileRepository(widget.api).binary.download(
        '/api/contacts/export',
        File(path!),
        limit: 1024 * 1024,
        isActive: () => active && revision == rev,
      ),
    );
  }

  Future<void> revoke(Map<String, dynamic> contact) async {
    final id = peer(contact);
    if (!RegExp(r'^[a-f0-9]{64}$').hasMatch(id)) return;
    if (!await confirm(
      'Отозвать доверие?',
      'Контакт останется в списке. Доверие карточке этого узла будет отозвано.',
    ))
      return;
    await mutate(() async {
      await widget.api.request('/api/peers/trusted/remove', body: {'id': id});
      if (!active) return;
      final remaining = ShellRepository.list(await repo.trusted(), 'peers');
      if (remaining.any((p) => p['id'] == id))
        throw const ApiFailure('business');
    });
  }

  Future<void> rename(Map<String, dynamic> contact) async {
    final rev = revision;
    final value = await showDialog<String>(
      context: context,
      builder: (_) => ContactNameDialog(initial: '${contact['name']}'),
    );
    if (!active || rev != revision || value == null || value.isEmpty) return;
    await mutate(() => repo.rename(contact, value));
  }

  Future<bool> confirm(String title, String explanation) async {
    final rev = revision;
    final answer = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: UiText(title),
        content: UiText(explanation),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const UiText('Отмена'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const UiText('Подтвердить'),
          ),
        ],
      ),
    );
    return active && rev == revision && answer == true;
  }

  Future<void> joinInvited(GroupInviteMessage inv) async {
    final rev = revision;
    final value = await showDialog<String>(
      context: context,
      builder: (_) => const ContactNameDialog(initial: '', joiningGroup: true),
    );
    if (!active || rev != revision || value == null || value.isEmpty) return;
    await mutate(() async {
      await widget.api.request(
        '/api/groups/${inv.gid}/join',
        body: {'name': value, if (inv.code.isNotEmpty) 'code': inv.code},
      );
    });
  }

  void editMessage(Map<String, dynamic> message) {
    // Правка в строке ввода с плашкой «Редактирование сообщения», как в телефоне.
    setState(() {
      editingId = '${message['msg_id']}';
      replyTo = null;
      composer.text = '${message['text'] ?? ''}';
      composer.selection = TextSelection.collapsed(
        offset: composer.text.length,
      );
    });
  }

  /// Переслать сообщения или файлы выбранным контактам (до 5 за раз, до 20 на сообщение за сеанс).
  Future<void> forward(List<Map<String, dynamic>> items) async {
    if (items.isEmpty || sending || selected == null) return;
    final here = peer(selected!);
    final to = await pickRecipients(
      context,
      title: items.length == 1
          ? 'Переслать сообщение'
          : 'Переслать сообщения (${items.length})',
      contacts: [
        for (final c in contacts)
          if (peer(c) != here) c,
      ],
    );
    if (to == null || to.isEmpty || !active) return;
    final targets = [for (final c in to) peer(c)];
    for (final m in items) {
      final all = {...?forwardedTo['${m['msg_id']}'], ...targets};
      if (all.length > 20) {
        setState(
          () => error =
              'Одно сообщение можно переслать не больше чем 20 получателям.',
        );
        return;
      }
    }
    final files = FileRepository(widget.api);
    await mutate(() async {
      for (final m in items) {
        final att = m['attachment'] is Map
            ? Map<String, dynamic>.from(m['attachment'] as Map)
            : null;
        final text = '${m['text'] ?? ''}';
        Uint8List? bytes;
        try {
          if (att != null) {
            final size = att['size'];
            if (size is! num || size <= 0) throw const ApiFailure('protocol');
            bytes = await files.binary.bytes(
              files.path(att, 'content'),
              limit: size.toInt(),
            );
          }
          for (var i = 0; i < to.length; i++) {
            if (!active) return;
            if (bytes != null) {
              await files.uploadBytes(
                targets[i],
                '${att!['filename']}',
                bytes,
                phone: to[i]['is_phone'] == true,
                isActive: () => active,
              );
            } else if (text.isNotEmpty) {
              await repo.send(targets[i], text);
            }
            (forwardedTo['${m['msg_id']}'] ??= <String>{}).add(targets[i]);
          }
        } finally {
          bytes?.fillRange(0, bytes.length, 0);
        }
      }
    }, success: () => picked.clear());
  }

  Future<void> deleteSelected() async {
    final target = peer(selected!), ids = picked.toList();
    if (ids.isEmpty) return;
    if (!await confirm(
      'Удалить выбранные сообщения (${ids.length})?',
      'Сообщения будут удалены только в вашей истории.',
    )) {
      return;
    }
    await mutate(() async {
      for (final id in ids) {
        await repo.deleteMessage(target, id);
      }
    }, success: () => picked.clear());
  }

  Future<void> deleteMessage(Map<String, dynamic> message) async {
    final target = peer(selected!);
    if (await confirm(
      'Удалить сообщение?',
      'Сообщение будет удалено только в вашей истории.',
    )) {
      await mutate(() => repo.deleteMessage(target, '${message['msg_id']}'));
    }
  }

  Future<void> clearHistory() async {
    final target = peer(selected!);
    if (await confirm(
      'Очистить историю чата?',
      'Будет очищена ваша история выбранного чата.',
    )) {
      await mutate(() => repo.clearHistory(target));
    }
  }

  Widget field(
    TextEditingController controller,
    String label, {
    int lines = 1,
  }) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 8),
    child: TextField(
      controller: controller,
      enabled: !sending,
      minLines: lines,
      maxLines: lines == 1 ? 1 : 6,
      decoration: InputDecoration(
        labelText: tr(context, label),
        border: const OutlineInputBorder(),
      ),
    ),
  );

  /// Собственная карточка и обмен списком контактов — в панели «Добавить» (отдельной вкладки «Контакты» больше нет).
  Widget ownCardPanel() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const UiText(
        'Карточка собственного узла',
        style: TextStyle(fontWeight: FontWeight.bold),
      ),
      if (ownCard != null) SelectableText(ownCard!),
      Wrap(
        spacing: 14,
        children: [
          CommandText(
            'Копировать карточку',
            onTap: sending || ownCard == null
                ? null
                : () => mutate(() async {
                    await Clipboard.setData(ClipboardData(text: ownCard!));
                  }),
          ),
          CommandText(
            'Экспорт contacts.json',
            onTap: sending ? null : exportContacts,
          ),
        ],
      ),
    ],
  );

  /// Известные узлу, но не добавленные в контакты (доверенные карточки): показываются только в поиске.
  List<String> strangers() {
    final q = search.text.trim().toLowerCase();
    if (q.length < 2) return const [];
    return [
      for (final t in trusted)
        if (t['id'] is String &&
            !contacts.any((c) => (t['id'] as String).startsWith('${c['short_id']}')) &&
            '${t['name'] ?? ''} ${t['id']}'.toLowerCase().contains(q))
          t['id'] as String,
    ];
  }

  Widget contactList() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      TextField(
        controller: search,
        onChanged: (_) => setState(() {}),
        decoration: InputDecoration(
          labelText: tr(context, 'Поиск по имени или ID'),
        ),
      ),
      if (contacts.isEmpty && !loading && strangers().isEmpty)
        const UiText('Контактов пока нет.'),
      for (final id in strangers())
        Container(
          margin: const EdgeInsets.only(bottom: 6),
          padding: const EdgeInsets.all(8),
          decoration: BoxDecoration(
            color: Icq.panel,
            border: Border.all(color: Icq.border),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const UiText(
                'Не в контактах',
                style: TextStyle(fontWeight: FontWeight.bold, fontSize: 13),
              ),
              Text(
                id.substring(0, 16),
                style: const TextStyle(fontSize: 12, color: Icq.dark),
              ),
              CommandText(
                'Добавить в контакты',
                onTap: sending
                    ? null
                    : () => mutate(() async {
                        await repo.saveContact(id);
                        search.clear();
                      }),
              ),
            ],
          ),
        ),
      for (final c in contacts.where(
        (c) => '${c['name']} ${c['short_id']}'.toLowerCase().contains(
          search.text.toLowerCase(),
        ),
      ))
        Container(
          margin: const EdgeInsets.only(bottom: 6),
          decoration: BoxDecoration(
            color: selected?['short_id'] == c['short_id']
                ? Icq.active
                : Icq.panel,
            border: Border.all(color: Icq.border),
          ),
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                InkWell(
                  onTap: sending ? null : () => choose(c),
                  child: Row(
                    children: [
                      ContactAvatar(
                        api: widget.api,
                        avatarId: '${c['avatar_id'] ?? c['short_id'] ?? ''}',
                        name: '${c['name']}'.replaceFirst(
                          RegExp('^[\\s\u{1F4F1}\u{1F4BB}\u{1F5A5}\uFE0F]+', unicode: true),
                          '',
                        ),
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            UiText(
                              '${c['name']} · ${tr(context, c['online'] == true ? 'В сети' : 'Не в сети')}',
                              style: const TextStyle(
                                fontWeight: FontWeight.bold,
                                fontSize: 14,
                              ),
                            ),
                            Text(
                              '${c['short_id']}',
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(
                                fontSize: 12,
                                color: Icq.dark,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
                TextButtonTheme(
                  data: TextButtonThemeData(
                    style: TextButton.styleFrom(
                      minimumSize: const Size(0, 26),
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      textStyle: const TextStyle(
                        fontSize: 12,
                        fontFamily: 'Tahoma',
                      ),
                    ),
                  ),
                  child: Wrap(
                    spacing: 4,
                    runSpacing: 2,
                    children: [
                      TextButton(
                        onPressed: sending ? null : () => choose(c),
                        child: const UiText('Открыть чат'),
                      ),
                      TextButton(
                        onPressed: sending ? null : () => rename(c),
                        child: const UiText('Переименовать'),
                      ),
                      if (c['is_phone'] == true) ...[
                        for (final a in ['remove', 'hide', 'block'])
                          TextButton(
                            onPressed: sending
                                ? null
                                : () async {
                                    if (!await confirm(
                                      'Изменить доступ устройства?',
                                      deviceActionLabel(a),
                                    ))
                                      return;
                                    await mutate(
                                      () => widget.api
                                          .request(
                                            '/api/mobile/phone-action',
                                            body: {
                                              'peer': c['short_id'],
                                              'action': a,
                                            },
                                          )
                                          .then((_) {}),
                                      success: () {
                                        selected = null;
                                        messages = [];
                                        composer.clear();
                                        drafts.clear();
                                        pickedFile = null;
                                      },
                                    );
                                  },
                            child: UiText(deviceActionLabel(a)),
                          ),
                      ],
                      if (c['is_phone'] != true &&
                          trusted.any((t) => t['id'] == peer(c)))
                        TextButton(
                          onPressed: sending ? null : () => revoke(c),
                          child: const UiText('Отозвать доверие'),
                        ),
                      TextButton(
                        onPressed: sending ? null : () => remove(c),
                        child: const UiText('Удалить контакт'),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      const SizedBox(height: 12),
      if (widget.page == 'Чат')
        DevicesPanel(
          api: widget.api,
          paired: false,
          busy: sending,
          isActive: () => active,
          mutate: (a) => mutate(a),
        ),
    ],
  );

  Widget contactImport() => PanelStack(
    children: [
      const UiText('Добавить по карточке узла', style: TextStyle(fontSize: 22)),
      field(name, 'Имя контакта'),
      field(card, 'Карточка YANDI-NODE-1', lines: 2),
      FilledButton(
        onPressed: sending
            ? null
            : () {
                if (name.text.trim().isEmpty ||
                    name.text.runes.length > 64 ||
                    !card.text.trim().startsWith('YANDI-NODE-1:') ||
                    card.text.length > 2048) {
                  setState(() {
                    error = 'Введите имя (до 64 символов) и карточку узла.';
                  });
                  return;
                }
                mutate(
                  () => repo.addCard(
                    card.text,
                    name.text,
                    isActive: () => active,
                  ),
                  success: () {
                    card.clear();
                    name.clear();
                  },
                );
              },
        child: const UiText('Добавить контакт'),
      ),
    ],
  );

  Widget peerPanel() => Panel(
    title: 'Профиль собеседника',
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (selected == null) ...[
          const Icon(Icons.person_outline, size: 64, color: Icq.header),
          const SizedBox(height: 16),
          const UiText('—'),
        ] else ...[
          ProfilePanel(
            key: ValueKey('profile:${selected!['short_id']}'),
            api: widget.api,
            peer: '${selected!['short_id']}',
            busy: sending,
            isActive: () => active,
            mutate: (action) => mutate(action),
          ),
          const SizedBox(height: 16),
          if (selected!['is_phone'] != true)
            ContactTunnel(
              key: ValueKey('tunnel:${selected!['short_id']}'),
              api: widget.api,
              peer: '${selected!['short_id']}',
              busy: sending,
              isActive: () => active,
              mutate: (action) => mutate(action),
            ),
        ],
      ],
    ),
  );

  Future<void> messageMenu(Map<String, dynamic> m, Offset at) async {
    if (sending || selected == null) return;
    final own = m['from'] == ownId;
    final commands = [
      if (own && m['attachment'] == null) commandEdit,
      commandReply,
      commandReact,
      if ('${m['text'] ?? ''}'.isNotEmpty) commandCopy,
      if ('${m['text'] ?? ''}'.isNotEmpty || m['attachment'] is Map)
        commandShare,
      commandSelect,
      if (own) commandDelete,
    ];
    final choice = await showMessageMenu(context, at, commands);
    if (!mounted || choice == null) return;
    switch (choice) {
      case commandReact:
        final emoji = await showDialog<String>(
          context: context,
          builder: (ctx) => SimpleDialog(
            title: const UiText('Смайл на сообщение'),
            children: [
              Wrap(
                alignment: WrapAlignment.center,
                spacing: 10,
                children: [
                  for (final e in const ['👍', '👎', '😊', '😢', '😂', '❤️'])
                    InkWell(
                      key: ValueKey('rx-$e'),
                      onTap: () => Navigator.pop(ctx, e),
                      child: Padding(
                        padding: const EdgeInsets.all(8),
                        child: Text(e, style: const TextStyle(fontSize: 28)),
                      ),
                    ),
                ],
              ),
            ],
          ),
        );
        if (!mounted || emoji == null || selected == null) return;
        final target = peer(selected!);
        await mutate(() => repo.react(target, '${m['msg_id']}', emoji));
      case commandEdit:
        editMessage(m);
      case commandShare:
        await forward([m]);
      case commandSelect:
        setState(() => picked.add('${m['msg_id']}'));
      case commandReply:
        setState(() {
          editingId = null;
          replyTo = {
            'msg_id': '${m['msg_id']}',
            'text': quoteExcerpt(
              '${m['text'] ?? ''}'.isEmpty
                  ? (m['attachment'] is Map
                        ? (m['attachment'] as Map)['filename']
                        : '')
                  : m['text'],
            ),
          };
        });
      case commandCopy:
        await Clipboard.setData(ClipboardData(text: '${m['text'] ?? ''}'));
      case commandDelete:
        await deleteMessage(m);
    }
  }

  String quoteAuthor(String id) {
    for (final m in messages) {
      if ('${m['msg_id']}' == id) {
        return m['from'] == ownId
            ? tr(context, 'Вы')
            : '${selected?['name'] ?? ''}';
      }
    }
    return '';
  }

  Widget bubble(Map<String, dynamic> m) {
    final own = m['from'] == ownId;
    final ts = DateTime.fromMillisecondsSinceEpoch(
      (m['timestamp'] as num).toInt(),
    ).toLocal();
    final hm =
        '${ts.hour.toString().padLeft(2, '0')}:${ts.minute.toString().padLeft(2, '0')}';
    final reply = m['reply_to'] is Map ? m['reply_to'] as Map : null;
    var tapAt = Offset.zero;
    return Align(
      key: anchors.putIfAbsent('${m['msg_id']}', () => GlobalKey()),
      alignment: own ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: (d) => tapAt = d.globalPosition,
          onTap: () {
            if (picked.isEmpty) {
              messageMenu(m, tapAt);
            } else {
              setState(() {
                final id = '${m['msg_id']}';
                if (!picked.remove(id)) picked.add(id);
              });
            }
          },
          child: Container(
            margin: const EdgeInsets.symmetric(vertical: 4),
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 4),
            decoration: BoxDecoration(
              color: own ? Icq.sent : Colors.white,
              border: Border.all(
                color: picked.contains('${m['msg_id']}')
                    ? const Color(0xFF2E86DE)
                    : (own ? Icq.mine : Icq.peer),
                width: picked.contains('${m['msg_id']}') ? 3 : 1.5,
              ),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                if (!own)
                  Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      InitialBadge('${selected!['name']}', size: 20),
                      const SizedBox(width: 6),
                      Text(
                        '${selected!['name']}',
                        style: const TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.bold,
                          color: Icq.peer,
                        ),
                      ),
                    ],
                  ),
                if (reply != null)
                  QuoteCard(
                    author: quoteAuthor('${reply['msg_id']}'),
                    text: '${reply['text'] ?? ''}',
                    onTap: () {
                      final ctx = anchors['${reply['msg_id']}']?.currentContext;
                      if (ctx != null) {
                        Scrollable.ensureVisible(
                          ctx,
                          duration: const Duration(milliseconds: 200),
                        );
                      }
                    },
                  ),
                if (GroupInviteMessage.tryParse('${m['text'] ?? ''}')
                    case final inv?)
                  GroupInviteCard(
                    invite: inv,
                    outgoing: own,
                    onJoin: () => joinInvited(inv),
                  )
                else
                  Text(stripLinks('${m['text'] ?? ''}')),
                if (m['attachment'] is Map)
                  AttachmentView(
                    key: ValueKey('${peer(selected!)}:${m['msg_id']}'),
                    api: widget.api,
                    attachment: Map<String, dynamic>.from(
                      m['attachment'] as Map,
                    ),
                    isActive: () => active,
                    mutate: (action) => mutate(action),
                    busy: sending,
                  ),
                if (m['reactions'] is Map &&
                    ('${(m['reactions'] as Map)['me'] ?? ''}'.isNotEmpty ||
                        '${(m['reactions'] as Map)['peer'] ?? ''}'.isNotEmpty))
                  Padding(
                    key: ValueKey('rx-row-${m['msg_id']}'),
                    padding: const EdgeInsets.only(top: 2),
                    child: Text(
                      [
                        for (final k in const ['me', 'peer'])
                          if ('${(m['reactions'] as Map)[k] ?? ''}'.isNotEmpty)
                            '${(m['reactions'] as Map)[k]}',
                      ].toSet().join(' '),
                      style: const TextStyle(fontSize: 16),
                    ),
                  ),
                Align(
                  alignment: Alignment.centerRight,
                  child: DeliveryMark(
                    time: hm,
                    status: m['status'],
                    own: own,
                    edited: m['edited'] == true,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  static final _link = RegExp(
    r'(https?://|www\.)[^\s<>"]+',
    caseSensitive: false,
  );

  /// Вид «Файлы чата»: картинки, файлы, аудио, видео и ссылки по разделам; сообщений здесь нет.
  List<Widget> filesList() {
    String kind(Map a) {
      final t = FileRepository.mime('${a['filename']}');
      return t.startsWith('image/')
          ? 'Картинки'
          : t.startsWith('audio/')
          ? 'Аудио'
          : t.startsWith('video/')
          ? 'Видео'
          : 'Файлы';
    }

    final tab = fileTabs[fileTab];
    final withFiles = [
      for (final m in messages)
        if (m['attachment'] is Map &&
            (tab == 'Все' || kind(m['attachment'] as Map) == tab))
          m,
    ];
    final links = <String>[
      if (tab == 'Все' || tab == 'Ссылки')
        for (final m in messages)
          if (m['attachment'] is! Map)
            for (final l in _link.allMatches('${m['text'] ?? ''}'))
              l.group(0)!.replaceAll(RegExp(r'[.,;:!?)\]]+$'), ''),
    ];
    return [
      Wrap(
        spacing: 14,
        children: [
          for (var i = 0; i < fileTabs.length; i++)
            InkWell(
              onTap: () => setState(() => fileTab = i),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 2),
                child: UiText(
                  fileTabs[i],
                  style: TextStyle(
                    fontSize: 13,
                    fontWeight: fileTab == i
                        ? FontWeight.bold
                        : FontWeight.normal,
                    color: fileTab == i ? Icq.active : Icq.header,
                    decoration: fileTab == i
                        ? TextDecoration.none
                        : TextDecoration.underline,
                  ),
                ),
              ),
            ),
        ],
      ),
      if (withFiles.isEmpty && links.isEmpty)
        const UiText('В этом разделе пусто'),
      for (final m in withFiles) bubble(m),
      for (final l in links)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 3),
          child: Row(
            children: [
              const Icon(Icons.link, size: 18, color: Icq.header),
              const SizedBox(width: 6),
              Expanded(child: SelectableText(l)),
              CommandText(
                'Копировать',
                onTap: () => Clipboard.setData(ClipboardData(text: l)),
              ),
            ],
          ),
        ),
    ];
  }

  Widget conversation({bool wide = false}) {
    if (selected == null)
      return Container(
        color: Colors.white,
        padding: const EdgeInsets.all(24),
        child: const Align(
          alignment: Alignment.topCenter,
          child: UiText(
            'Выберите контакт для переписки.',
            style: TextStyle(color: Icq.dark, fontStyle: FontStyle.italic),
          ),
        ),
      );
    final history = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        UiText(
          'Чат: ${selected!['name']}',
          style: const TextStyle(fontSize: 20, fontWeight: FontWeight.bold),
        ),
        Wrap(
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: 14,
          children: [
            if (widget.calls != null) ...[
              CommandText(
                'Позвонить',
                onTap: () => widget.calls!.start(
                  peer(selected!),
                  phone: selected!['is_phone'] == true,
                ),
              ),
              CommandText(
                'Видеозвонок',
                onTap: () => widget.calls!.start(
                  peer(selected!),
                  video: true,
                  phone: selected!['is_phone'] == true,
                ),
              ),
            ],
            CommandText(
              filesView ? 'Назад к сообщениям' : 'Файлы чата',
              onTap: () => setState(() {
                filesView = !filesView;
                fileTab = 0;
              }),
            ),
            CommandText(
              'Очистить историю',
              onTap: sending ? null : clearHistory,
            ),
            const UiText(
              'Показаны последние 100 сообщений не старше года.',
              style: TextStyle(fontSize: 12, color: Icq.dark),
            ),
          ],
        ),
        if (picked.isNotEmpty)
          Wrap(
            spacing: 14,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              UiText(
                'Выбрано: ${picked.length}',
                style: const TextStyle(fontWeight: FontWeight.bold),
              ),
              CommandText(
                'Переслать выбранные',
                onTap: sending
                    ? null
                    : () => forward([
                        for (final m in messages)
                          if (picked.contains('${m['msg_id']}')) m,
                      ]),
              ),
              CommandText(
                'Удалить выбранные',
                onTap: sending ? null : deleteSelected,
              ),
              CommandText('Снять выбор', onTap: () => setState(picked.clear)),
            ],
          ),
        if (filesView)
          ...filesList()
        else ...[
          if (!loading && messages.isEmpty && error == null)
            const UiText('История пуста.'),
          for (final m in messages) bubble(m),
        ],
      ],
    );
    final inputField = Focus(
      onKeyEvent: (_, event) {
        if (event is KeyDownEvent &&
            event.logicalKey == LogicalKeyboardKey.enter &&
            !HardwareKeyboard.instance.isShiftPressed) {
          send();
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      },
      child: field(composer, 'Сообщение', lines: 2),
    );
    final attachButton = ComposerTools(
      onAttach: sending || choosingFile ? null : pickFile,
      controller: composer,
    );
    final sendButton = SendOrMic(
      controller: composer,
      sendTip: 'Отправить',
      hasFile: pickedFile != null,
      onSend: sending
          ? null
          : (pickedFile != null && composer.text.trim().isEmpty
                ? sendFile
                : send),
      onMic: sending || editingId != null ? null : startVoice,
    );
    final input = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (editingId != null)
          Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: Row(
              children: [
                const Icon(Icons.edit_outlined, size: 16, color: Icq.header),
                const SizedBox(width: 6),
                const Expanded(
                  child: UiText(
                    'Редактирование сообщения',
                    style: TextStyle(color: Icq.header),
                  ),
                ),
                CommandText(
                  'Отмена',
                  onTap: () => setState(() {
                    editingId = null;
                    composer.clear();
                  }),
                ),
              ],
            ),
          ),
        if (replyTo != null)
          QuoteCard(
            author: quoteAuthor('${replyTo!['msg_id']}'),
            text: '${replyTo!['text']}',
            trailing: CommandText(
              'Отмена',
              onTap: () => setState(() => replyTo = null),
            ),
          ),
        if (failedVoice != null && !voice.active && selected != null && failedVoicePeer == peer(selected!))
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              const UiText('Голосовое не отправлено.'),
              TextButton(onPressed: sending ? null : retryVoice, child: const UiText('Повторить отправку')),
              TextButton(onPressed: () => setState(() => failedVoice = null), child: const UiText('Удалить запись')),
            ],
          ),
        if (voice.active)
          VoiceBar(
            seconds: voice.seconds,
            onCancel: () => finishVoice(send: false),
            onSend: () => finishVoice(send: true),
          )
        else
          LayoutBuilder(
            builder: (context, box) => box.maxWidth < 520
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      inputField,
                      Wrap(children: [attachButton, sendButton]),
                    ],
                  )
                : Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Padding(
                        padding: const EdgeInsets.only(top: 8),
                        child: attachButton,
                      ),
                      const SizedBox(width: 6),
                      Expanded(child: inputField),
                      const SizedBox(width: 6),
                      Padding(
                        padding: const EdgeInsets.only(top: 8),
                        child: sendButton,
                      ),
                    ],
                  ),
          ),
        if (pickedFile != null)
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text(pickedFile!.uri.pathSegments.last),
              TextButton(
                onPressed: sending
                    ? null
                    : () => setState(() {
                        pickedFile = null;
                      }),
                child: const UiText('Убрать файл'),
              ),
              FilledButton(
                onPressed: sending ? null : sendFile,
                child: const UiText('Отправить файл'),
              ),
            ],
          ),
        if (transferSize > 0) ...[
          LinearProgressIndicator(value: transferred / transferSize),
          UiText(
            'Передано демону: $transferred / $transferSize байт. Доставка подтверждается отдельно.',
          ),
        ],
      ],
    );
    if (!wide)
      return Panel(
        color: Colors.white,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [history, input, peerPanel()],
        ),
      );
    return Container(
      decoration: const BoxDecoration(color: Colors.white, border: Icq.frame),
      child: Column(
        children: [
          Expanded(
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(12),
              child: history,
            ),
          ),
          Container(
            decoration: const BoxDecoration(
              color: Icq.background,
              border: Border(top: BorderSide(color: Icq.border)),
            ),
            padding: const EdgeInsets.all(10),
            child: input,
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) => widget.page == 'О приложении'
      ? const AboutPanel()
      : widget.page == 'Жалобы'
      ? ComplaintsFeature(
          api: widget.api,
          isActive: widget.isActive,
          onFailure: widget.onFailure,
        )
      : widget.page == 'Поиск'
      ? SearchFeature(
          api: widget.api,
          isActive: widget.isActive,
          onFailure: widget.onFailure,
        )
      : widget.page == 'Шлюзы'
      ? NetworkFeature(
          api: widget.api,
          page: widget.page,
          isActive: widget.isActive,
          onFailure: widget.onFailure,
          onLocked: widget.onLocked,
        )
      : widget.page == 'Группы'
      ? GroupsFeature(
          api: widget.api,
          onOpened: widget.onGroupOpened,
          isActive: widget.isActive,
          onFailure: widget.onFailure,
          onLocked: widget.onLocked,
        )
      : Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (loading || sending) const LinearProgressIndicator(),
            if (error != null) NoticePanel(error!, error: true),
            if (notice != null) NoticePanel(notice!),
            if (refreshError) const UiText('Данные не обновлены.'),
            if (widget.page == 'Настройки') ...[
              AdvancedSettings(
                api: widget.api,
                busy: sending,
                isActive: () => active,
                mutate: (a) => mutate(a),
              ),
              DevicesPanel(
                api: widget.api,
                paired: true,
                busy: sending,
                isActive: () => active,
                mutate: (a) => mutate(a),
              ),
              PanelStack(
                children: [
                  const UiText('Профиль', style: TextStyle(fontSize: 22)),
                  ProfilePanel(
                    api: widget.api,
                    busy: sending,
                    isActive: () => active,
                    mutate: (action) => mutate(action),
                  ),
                  field(name, 'Имя профиля'),
                  FilledButton(
                    onPressed: sending || loading
                        ? null
                        : () =>
                              mutate(() => repo.saveProfile(name.text.trim())),
                    child: const UiText('Сохранить профиль'),
                  ),
                ],
              ),
            ] else ...[
              LayoutBuilder(
                builder: (context, size) {
                  final contactsPanel = Panel(
                    title: 'Контакты',
                    trailing: widget.page == 'Чат'
                        ? BevelButton(
                            label: 'Добавить',
                            onPressed: sending
                                ? null
                                : () => setState(
                                    () => addingContact = !addingContact,
                                  ),
                          )
                        : null,
                    padding: const EdgeInsets.all(8),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (addingContact) ownCardPanel(),
                        if (addingContact) contactImport(),
                        contactList(),
                      ],
                    ),
                  );
                  if (widget.page != 'Чат')
                    return Column(
                      children: [
                        contactsPanel,
                        if (selected != null) conversation(),
                      ],
                    );
                  if (size.maxWidth < 700)
                    return Column(children: [contactsPanel, conversation()]);
                  return SizedBox(
                    height: (MediaQuery.sizeOf(context).height - 185).clamp(
                      380.0,
                      900.0,
                    ),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        SizedBox(
                          width: size.maxWidth < 1000 ? 220 : 280,
                          child: SingleChildScrollView(child: contactsPanel),
                        ),
                        const SizedBox(width: 4),
                        Expanded(child: conversation(wide: true)),
                        const SizedBox(width: 4),
                        SizedBox(
                          width: size.maxWidth < 1000 ? 200 : 258,
                          child: SingleChildScrollView(child: peerPanel()),
                        ),
                      ],
                    ),
                  );
                },
              ),
            ],
          ],
        );
}

class ContactNameDialog extends StatefulWidget {
  final String initial;
  final bool editingMessage;
  final bool joiningGroup;
  const ContactNameDialog({
    super.key,
    required this.initial,
    this.editingMessage = false,
    this.joiningGroup = false,
  });
  @override
  State<ContactNameDialog> createState() => _ContactNameDialogState();
}

class _ContactNameDialogState extends State<ContactNameDialog> {
  late final field = TextEditingController(text: widget.initial);
  @override
  void dispose() {
    field.clear();
    field.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: UiText(
      widget.joiningGroup
          ? 'Вступить в группу'
          : widget.editingMessage
          ? 'Изменить сообщение'
          : 'Переименовать контакт',
    ),
    content: TextField(
      controller: field,
      maxLength: widget.editingMessage ? null : 64,
      maxLines: widget.editingMessage ? 6 : 1,
      decoration: InputDecoration(
        labelText: tr(
          context,
          widget.editingMessage ? 'Текст сообщения' : 'Имя',
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: () => Navigator.pop(context, field.text.trim()),
        child: const UiText('Сохранить'),
      ),
    ],
  );
}

String deviceActionLabel(String action) => const {
  'remove': 'Убрать устройство из контактов',
  'hide': 'Скрыть устройство',
  'block': 'Заблокировать устройство',
}[action]!;
