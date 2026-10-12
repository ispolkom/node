import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'domain/shell_repository.dart';
import 'domain/states.dart';
import 'transport/api.dart';
import 'groups.dart';

/// This subtree is disposed on lock, hide, disconnect and navigation.
class ShellFeatures extends StatefulWidget {
  final Api api;
  final String page;
  final void Function(ApiFailure) onFailure;
  final VoidCallback onLocked;
  final bool Function() isActive;
  const ShellFeatures({
    super.key,
    required this.api,
    required this.page,
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
  String? ownId, error, notice;
  bool loading = true, refreshing = false, sending = false;
  int revision = 0;
  bool refreshError = false;
  Timer? timer;
  bool get active => mounted && widget.isActive();

  @override
  void initState() {
    super.initState();
    if (widget.page == 'Группы') return;
    refresh();
    if (widget.page != 'Настройки') {
      timer = Timer.periodic(const Duration(seconds: 5), (_) => refresh());
    }
  }

  @override
  void dispose() {
    revision++;
    timer?.cancel();
    drafts.clear();
    contacts = messages = trusted = [];
    selected = null;
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
        name.text = '${profile['display_name'] ?? ''}';
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
          ownId = status['node_id'] as String?;
        });
        if (currentPeer != null && selected != null) {
          final history = await repo.history(currentPeer);
          if (!active || rev != revision) return;
          setState(() {
            messages = history;
          });
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

  Future<void> remove(Map<String, dynamic> contact) async {
    final rev = revision;
    final answer = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('Удалить контакт «${contact['name']}»?'),
        content: const Text(
          'Удаляется запись контакта. Доверие карточке и доступ телефона сохраняются.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Отмена'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Удалить'),
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

  Future<void> rename(Map<String, dynamic> contact) async {
    final rev = revision;
    final value = await showDialog<String>(
      context: context,
      builder: (_) => ContactNameDialog(initial: '${contact['name']}'),
    );
    if (!active || rev != revision || value == null || value.isEmpty) return;
    await mutate(() => repo.rename(contact, value));
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
      if (contacts.isEmpty && !loading) const Text('Контактов пока нет.'),
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
                      child: const Text('Открыть чат'),
                    ),
                    TextButton(
                      onPressed: sending ? null : () => rename(c),
                      child: const Text('Переименовать'),
                    ),
                    TextButton(
                      onPressed: sending ? null : () => remove(c),
                      child: const Text('Удалить контакт'),
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
        const Text('Выберите контакт для переписки.')
      else ...[
        Text('Чат: ${selected!['name']}', style: const TextStyle(fontSize: 22)),
        const Text('Показаны последние 100 сообщений не старше года.'),
        if (!loading && messages.isEmpty && error == null)
          const Text('История пуста.'),
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
                  if (m['attachment'] is Map)
                    const Text('Вложение (открытие файлов пока недоступно)'),
                  Text(
                    '${DateTime.fromMillisecondsSinceEpoch((m['timestamp'] as num).toInt()).toLocal()} · ${m['status'] ?? 'Неизвестный статус'}${m['edited'] == true ? ' · изменено' : ''}',
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
        FilledButton(
          onPressed: sending ? null : send,
          child: const Text('Отправить'),
        ),
      ],
    ],
  );

  @override
  Widget build(BuildContext context) => widget.page == 'Группы'
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
            if (refreshError) const Text('Данные не обновлены.'),
            if (widget.page == 'Настройки') ...[
              const Text('Профиль', style: TextStyle(fontSize: 22)),
              field(name, 'Имя профиля'),
              FilledButton(
                onPressed: sending || loading
                    ? null
                    : () => mutate(() => repo.saveProfile(name.text.trim())),
                child: const Text('Сохранить профиль'),
              ),
            ] else ...[
              if (widget.page == 'Контакты') ...[
                const Text(
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
                  child: const Text('Добавить контакт'),
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
  const ContactNameDialog({super.key, required this.initial});
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
    title: const Text('Переименовать контакт'),
    content: TextField(
      controller: field,
      maxLength: 64,
      decoration: const InputDecoration(labelText: 'Имя'),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Отмена'),
      ),
      FilledButton(
        onPressed: () => Navigator.pop(context, field.text.trim()),
        child: const Text('Сохранить'),
      ),
    ],
  );
}
