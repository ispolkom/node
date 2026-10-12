import 'preferences.dart';
import 'dart:io';
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
import 'profile_panel.dart';
import 'contact_tunnel.dart';
import 'devices.dart';
import 'advanced_settings.dart';

/// This subtree is disposed on lock, hide, disconnect and navigation.
class ShellFeatures extends StatefulWidget {
  final Api api;
  final String page;
  final AppPreferences? preferences;
  final void Function(ApiFailure) onFailure;
  final VoidCallback onLocked;
  final bool Function() isActive;
  const ShellFeatures({
    super.key,
    required this.api,
    required this.page,
    this.preferences,
    required this.onFailure,
    required this.onLocked,
    required this.isActive,
  });
  @override
  State<ShellFeatures> createState() => _ShellFeaturesState();
}

class _ShellFeaturesState extends State<ShellFeatures> {
  late final repo = ShellRepository(widget.api);
  final card = TextEditingController(), name = TextEditingController();
  final search = TextEditingController(), composer = TextEditingController();
  final drafts = <String, String>{};
  List<Map<String, dynamic>> contacts = [], messages = [], trusted = [];
  Map<String, dynamic>? selected;
  String? ownId, ownCard, lastProfileName, error, notice;
  bool loading = true, refreshing = false, sending = false;
  int revision = 0;
  File? pickedFile;
  int transferred = 0, transferSize = 0;
  bool choosingFile = false;
  bool refreshError = false;
  Timer? timer;
  bool get active => mounted && widget.isActive();

  @override
  void initState() {
    super.initState();
    if (const [
      'Группы',
      'Шлюзы',
      'Relay',
      'Звонки',
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
          if (!active || rev != revision) return;
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
    if (!active || sending) return;
    if (selected != null) drafts[peer(selected!)] = composer.text;
    revision++;
    setState(() {
      pickedFile = null;
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
    final target = peer(contact), text = composer.text;
    await mutate(
      () => repo.send(target, text),
      success: () {
        composer.clear();
        drafts.remove(target);
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
        title: Text('Удалить контакт «${contact['name']}»?'),
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
        title: Text(title),
        content: Text(explanation),
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

  Future<void> editMessage(Map<String, dynamic> message) async {
    final rev = revision, target = peer(selected!);
    final value = await showDialog<String>(
      context: context,
      builder: (_) => ContactNameDialog(
        initial: '${message['text'] ?? ''}',
        editingMessage: true,
      ),
    );
    if (!active || rev != revision || value == null || value.trim().isEmpty)
      return;
    await mutate(() => repo.edit(target, '${message['msg_id']}', value));
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
        labelText: label,
        border: const OutlineInputBorder(),
      ),
    ),
  );

  Widget contactList() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      TextField(
        controller: search,
        onChanged: (_) => setState(() {}),
        decoration: const InputDecoration(labelText: 'Поиск по имени или ID'),
      ),
      if (widget.page == 'Чат')
        DevicesPanel(
          api: widget.api,
          paired: false,
          busy: sending,
          isActive: () => active,
          mutate: (a) => mutate(a),
        ),
      if (contacts.isEmpty && !loading) const UiText('Контактов пока нет.'),
      for (final c in contacts.where(
        (c) => '${c['name']} ${c['short_id']}'.toLowerCase().contains(
          search.text.toLowerCase(),
        ),
      ))
        Card(
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${c['name']} · ${c['online'] == true ? 'В сети' : 'Не в сети'}',
                ),
                SelectableText('${c['short_id']}'),
                Wrap(
                  spacing: 8,
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
                                      revision++;
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
              ],
            ),
          ),
        ),
    ],
  );

  Widget conversation() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      if (selected == null)
        const UiText('Выберите контакт для переписки.')
      else ...[
        Text('Чат: ${selected!['name']}', style: const TextStyle(fontSize: 22)),
        if (selected!['is_phone'] != true)
          ContactTunnel(
            key: ValueKey('tunnel:${selected!['short_id']}'),
            api: widget.api,
            peer: '${selected!['short_id']}',
            busy: sending,
            isActive: () => active,
            mutate: (action) => mutate(action),
          ),
        ProfilePanel(
          key: ValueKey('profile:${selected!['short_id']}'),
          api: widget.api,
          peer: '${selected!['short_id']}',
          busy: sending,
          isActive: () => active,
          mutate: (action) => mutate(action),
        ),
        TextButton(
          onPressed: sending ? null : clearHistory,
          child: const UiText('Очистить историю'),
        ),
        const UiText('Показаны последние 100 сообщений не старше года.'),
        if (!loading && messages.isEmpty && error == null)
          const UiText('История пуста.'),
        for (final m in messages)
          Card(
            color: m['from'] == ownId
                ? const Color(0xffc4f3e5)
                : const Color(0xffffe2d7),
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(m['from'] == ownId ? 'Вы' : '${selected!['name']}'),
                  SelectableText('${m['text'] ?? ''}'),
                  if (m['from'] == ownId)
                    Wrap(
                      children: [
                        if (m['attachment'] == null)
                          TextButton(
                            onPressed: sending ? null : () => editMessage(m),
                            child: const UiText('Изменить сообщение'),
                          ),
                        TextButton(
                          onPressed: sending ? null : () => deleteMessage(m),
                          child: const UiText('Удалить сообщение'),
                        ),
                      ],
                    ),
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
                  Text(
                    '${DateTime.fromMillisecondsSinceEpoch((m['timestamp'] as num).toInt()).toLocal()} · ${ShellRepository.delivery(m['status'])}${m['edited'] == true ? ' · изменено' : ''}',
                  ),
                ],
              ),
            ),
          ),
        Focus(
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
        ),
        Wrap(
          children: [
            TextButton(
              onPressed: sending || choosingFile ? null : pickFile,
              child: const UiText('Выбрать файл'),
            ),
            if (pickedFile != null) ...[
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
          ],
        ),
        if (transferSize > 0) ...[
          LinearProgressIndicator(value: transferred / transferSize),
          Text(
            'Передано демону: $transferred / $transferSize байт. Доставка подтверждается отдельно.',
          ),
        ],
        FilledButton(
          onPressed: sending ? null : send,
          child: const UiText('Отправить'),
        ),
      ],
    ],
  );

  @override
  Widget build(BuildContext context) => widget.page == 'О приложении'
      ? const AboutPanel()
      : widget.page == 'Звонки'
      ? CallsFeature(
          api: widget.api,
          isActive: widget.isActive,
          onLocked: widget.onLocked,
          onFailure: widget.onFailure,
        )
      : const ['Шлюзы', 'Relay'].contains(widget.page)
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
          isActive: widget.isActive,
          onFailure: widget.onFailure,
          onLocked: widget.onLocked,
        )
      : Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (loading || sending) const LinearProgressIndicator(),
            if (error != null)
              Text(error!, style: const TextStyle(color: Colors.red)),
            if (notice != null) Text(notice!),
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
                    : () => mutate(() => repo.saveProfile(name.text.trim())),
                child: const UiText('Сохранить профиль'),
              ),
            ] else ...[
              if (widget.page == 'Контакты') ...[
                const UiText(
                  'Карточка собственного узла',
                  style: TextStyle(fontSize: 22),
                ),
                if (ownCard != null) SelectableText(ownCard!),
                Wrap(
                  children: [
                    TextButton(
                      onPressed: sending || ownCard == null
                          ? null
                          : () => mutate(() async {
                              await Clipboard.setData(
                                ClipboardData(text: ownCard!),
                              );
                            }),
                      child: const UiText('Копировать карточку'),
                    ),
                    TextButton(
                      onPressed: sending ? null : exportContacts,
                      child: const UiText('Экспорт contacts.json'),
                    ),
                  ],
                ),
                const UiText(
                  'Добавить по карточке узла',
                  style: TextStyle(fontSize: 22),
                ),
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
                              error =
                                  'Введите имя (до 64 символов) и карточку узла.';
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
              LayoutBuilder(
                builder: (context, size) {
                  if (size.maxWidth < 700)
                    return Column(children: [contactList(), conversation()]);
                  return Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(child: contactList()),
                      const SizedBox(width: 24),
                      Expanded(flex: 2, child: conversation()),
                    ],
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
  const ContactNameDialog({
    super.key,
    required this.initial,
    this.editingMessage = false,
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
    title: Text(
      widget.editingMessage ? 'Изменить сообщение' : 'Переименовать контакт',
    ),
    content: TextField(
      controller: field,
      maxLength: widget.editingMessage ? null : 64,
      maxLines: widget.editingMessage ? 6 : 1,
      decoration: InputDecoration(
        labelText: widget.editingMessage ? 'Текст сообщения' : 'Имя',
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
