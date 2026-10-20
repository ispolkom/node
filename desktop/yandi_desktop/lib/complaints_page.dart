import 'dart:async';
import 'package:flutter/material.dart';
import 'preferences.dart';
import 'transport/api.dart';
import 'groups.dart';
import 'theme.dart';

/// Страница «Жалобы»: слева обращения (кто пожаловался и на кого), справа текст, переписка и меры владельца узла.
/// Ответы уходят от имени узла («Администрация узла»), не от владельца лично.
class ComplaintsFeature extends StatefulWidget {
  final Api api;
  final bool Function() isActive;
  final void Function(ApiFailure failure) onFailure;
  const ComplaintsFeature({super.key, required this.api, required this.isActive, required this.onFailure});
  @override
  State<ComplaintsFeature> createState() => _ComplaintsFeatureState();
}

class _ComplaintsFeatureState extends State<ComplaintsFeature> {
  List<Map<String, dynamic>> items = [];
  List<Map<String, dynamic>> pairRequests = [];
  Map<String, dynamic>? current;
  String? selected, error, notice;
  bool loading = true, busy = false;
  String replySide = 'reporter';
  final TextEditingController text = TextEditingController();
  final TextEditingController note = TextEditingController();
  final TextEditingController hours = TextEditingController(text: '24');
  Timer? timer;
  bool get active => mounted && widget.isActive();

  static const reasonNames = {
    'spam': 'Спам или навязчивость',
    'abuse': 'Оскорбления, травля',
    'illegal': 'Запрещённое содержимое',
    'danger': 'Угрозы, опасность',
    'other': 'Другое',
  };
  static const statusNames = {'new': 'Новая', 'review': 'В работе', 'closed': 'Закрыта'};

  @override
  void initState() {
    super.initState();
    refresh();
    timer = Timer.periodic(const Duration(seconds: 10), (_) => refresh());
  }

  @override
  void dispose() {
    timer?.cancel();
    text.dispose();
    note.dispose();
    hours.dispose();
    super.dispose();
  }

  static List<Map<String, dynamic>> listOf(Object? v) =>
      v is List ? [for (final x in v) if (x is Map) x.cast<String, dynamic>()] : const [];

  Future<void> refresh() async {
    if (!active || busy) return;
    try {
      final r = await widget.api.request('/api/complaints');
      Map<String, dynamic>? cur;
      final id = selected;
      if (id != null) {
        cur = await widget.api.request('/api/complaints/$id');
      }
      // запросы сопряжения: отдельный блок, его сбой не мешает жалобам
      List<Map<String, dynamic>> pairs = pairRequests;
      try {
        pairs = listOf((await widget.api.request('/api/pair-requests'))['requests']);
      } catch (_) {}
      if (!active) return;
      setState(() {
        pairRequests = pairs;
        items = listOf(r['complaints']);
        current = cur;
        loading = false;
        error = null;
      });
    } catch (e) {
      if (!active) return;
      setState(() {
        loading = false;
        error = e is ApiFailure ? e.message : 'Не удалось получить жалобы. Если закрытая среда заперта, войдите.';
      });
    }
  }

  /// Мера, уничтожающая группы: пароль узла спрашивается заново (как при прямом удалении группы). [ask] — спросить сразу;
  /// иначе — только если узел потребует (устройство владеет группами).
  Future<void> stepUpMeasure(Map<String, dynamic> c, String kind, {required bool ask}) async {
    Future<void> send([String? password]) => widget.api
        .request('/api/complaints/${c['id']}/measure',
            body: {'kind': kind, 'note': note.text, if (password != null) 'password': password})
        .then((_) {});
    Future<String?> askPassword() async {
      final v = await showDialog<List<String>>(
        context: context,
        builder: (_) => const GroupForm(
          title: 'Эта мера удаляет группы. Пароль входа узла',
          labels: ['Пароль входа узла'],
          secret: true,
        ),
      );
      return v == null || v.isEmpty ? null : v[0];
    }
    if (ask) {
      final pw = await askPassword();
      if (pw == null) return;
      return send(pw);
    }
    try {
      await send();
    } on ApiFailure catch (e) {
      if (e.status != 403) rethrow;
      final pw = await askPassword();
      if (pw == null) return;
      await send(pw);
    }
  }

  Future<void> act(Future<void> Function() f, {String? ok}) async {
    setState(() { busy = true; notice = null; });
    try {
      await f();
      if (ok != null && active) setState(() => notice = ok);
    } catch (e) {
      if (active) setState(() => notice = 'Не получилось: ${e is ApiFailure ? e.message : e}');
    } finally {
      if (active) setState(() => busy = false);
    }
    await refresh();
  }

  String when(Object? ms) {
    final t = DateTime.fromMillisecondsSinceEpoch((ms as num?)?.toInt() ?? 0);
    two(int n) => n.toString().padLeft(2, '0');
    return '${two(t.day)}.${two(t.month)} ${two(t.hour)}:${two(t.minute)}';
  }

  Widget listPane() => SizedBox(
    width: 300,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        UiText('Обращения (${items.length})', style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
        if (items.isEmpty && !loading) const UiText('Жалоб нет.'),
        for (final c in items)
          Card(
            color: selected == c['id'] ? const Color(0xffe2f0d8) : null,
            child: ListTile(
              dense: true,
              onTap: () {
                setState(() => selected = '${c['id']}');
                refresh();
              },
              title: Text('${c['target_kind'] == 'group' ? 'Группа' : 'Пользователь'}: ${c['target_name'] ?? ''}'.trim()),
              subtitle: Text('${when(c['ts'])} · ${statusNames[c['status']] ?? c['status']} · ${reasonNames[c['reason']] ?? ''}'),
              trailing: ('${c['status']}' == 'new') ? const Icon(Icons.circle, size: 10, color: Colors.red) : null,
            ),
          ),
      ],
    ),
  );

  Widget detailPane() {
    final c = current;
    if (c == null) return const Expanded(child: Padding(padding: EdgeInsets.all(16), child: UiText('Выберите жалобу слева.')));
    final isUser = c['target_kind'] == 'user';
    final thread = listOf(c['thread']);
    final measures = listOf(c['measures']);
    final evidence = (c['evidence'] as List?)?.cast<String>() ?? const <String>[];
    return Expanded(
      child: Padding(
        padding: const EdgeInsets.only(left: 16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            UiText('${isUser ? 'Пользователь' : 'Группа'}: ${c['target_name'] ?? ''}', style: const TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
            Text('Заявитель: ${('${c['reporter']}').substring(0, 8)}… · ${when(c['ts'])} · ${reasonNames[c['reason']] ?? ''} · ${statusNames[c['status']] ?? ''}'),
            const SizedBox(height: 8),
            SelectableText('${c['text']}'),
            if (evidence.isNotEmpty) ...[
              const SizedBox(height: 8),
              const UiText('Приложено заявителем:', style: TextStyle(fontWeight: FontWeight.bold)),
              for (final e in evidence) Padding(padding: const EdgeInsets.only(left: 8, top: 2), child: SelectableText('• $e')),
            ],
            const Divider(),
            const UiText('Переписка', style: TextStyle(fontWeight: FontWeight.bold)),
            if (thread.isEmpty) const UiText('Пока пусто.'),
            for (final m in thread)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 2),
                child: Text('${when(m['ts'])} · ${m['side'] == 'target' ? 'нарушителю' : 'заявителю'} · ${m['from'] == 'admin' ? 'Администрация узла' : 'пользователь'}: ${m['text']}'),
              ),
            Row(
              children: [
                DropdownButton<String>(
                  value: replySide,
                  items: [
                    const DropdownMenuItem(value: 'reporter', child: UiText('Заявителю')),
                    if (isUser) const DropdownMenuItem(value: 'target', child: UiText('Нарушителю')),
                  ],
                  onChanged: (v) => setState(() => replySide = (v == 'target' && !isUser) ? 'reporter' : v ?? 'reporter'),
                ),
                const SizedBox(width: 8),
                Expanded(child: TextField(controller: text, decoration: const InputDecoration(hintText: 'Ответ от имени узла'))),
                TextButton(
                  onPressed: busy ? null : () => act(() async {
                    final t = text.text.trim();
                    if (t.isEmpty) return;
                    await widget.api.request('/api/complaints/${c['id']}/reply', body: {'side': replySide, 'text': t});
                    text.clear();
                  }, ok: 'Ответ отправлен от имени узла.'),
                  child: const UiText('Отправить'),
                ),
              ],
            ),
            const Divider(),
            const UiText('Меры', style: TextStyle(fontWeight: FontWeight.bold)),
            for (final m in measures)
              Text('${when(m['ts'])} · ${m['kind']} ${m['note'] ?? ''}'),
            Wrap(
              spacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                SizedBox(width: 220, child: TextField(controller: note, decoration: const InputDecoration(hintText: 'Причина или текст'))),
                if (isUser) ...[
                  TextButton(
                    onPressed: busy ? null : () => act(() => widget.api.request('/api/complaints/${c['id']}/measure', body: {'kind': 'warn', 'note': note.text}).then((_) {}), ok: 'Предупреждение отправлено.'),
                    child: const UiText('Предупреждение'),
                  ),
                  SizedBox(width: 56, child: TextField(controller: hours, decoration: const InputDecoration(hintText: 'часов'))),
                  TextButton(
                    onPressed: busy ? null : () => act(() => widget.api.request('/api/complaints/${c['id']}/measure', body: {'kind': 'restrict', 'note': note.text, 'hours': int.tryParse(hours.text) ?? 24}).then((_) {}), ok: 'Ограничение включено.'),
                    child: const UiText('Ограничить'),
                  ),
                  TextButton(
                    onPressed: busy ? null : () => act(() => stepUpMeasure(c, 'remove', ask: false), ok: 'Устройство удалено с узла.'),
                    child: const UiText('Удалить с узла'),
                  ),
                ] else
                  TextButton(
                    onPressed: busy ? null : () => act(() => stepUpMeasure(c, 'delete_group', ask: true), ok: 'Группа удалена с узла.'),
                    child: const UiText('Удалить группу с узла'),
                  ),
                TextButton(
                  onPressed: busy ? null : () => act(() => widget.api.request('/api/complaints/${c['id']}/status', body: {'status': 'closed'}).then((_) {}), ok: 'Жалоба закрыта.'),
                  child: const UiText('Закрыть жалобу'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  /// Гость без QR просит принять его на узел. Принять = «пользователь узла» (владельцем устройство становится только по QR).
  Widget pairBlock() => Padding(
    padding: const EdgeInsets.only(bottom: 16),
    child: Panel(
      title: 'Запросы сопряжения',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const UiText('Устройства, подключённые без QR, просят принять их на узел. Принятое устройство получает права пользователя узла; владельцем оно может стать только по QR.'),
          const SizedBox(height: 8),
          if (pairRequests.isEmpty) const UiText('Запросов нет.'),
          for (final r in pairRequests)
            Container(
              margin: const EdgeInsets.only(bottom: 6),
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              decoration: BoxDecoration(color: Colors.white, border: Border.all(color: Icq.border)),
              child: Row(
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '${tr(context, 'Гость')} ${(r['nick'] is String && (r['nick'] as String).trim().isNotEmpty) ? r['nick'] : r['name']} · ${tr(context, 'метка')} ${r['mark']} · ${when(r['created_ms'])}',
                          style: const TextStyle(fontSize: 14, fontStyle: FontStyle.italic),
                        ),
                        if (r['text'] is String && (r['text'] as String).trim().isNotEmpty)
                          Padding(
                            padding: const EdgeInsets.only(top: 4),
                            child: SelectableText('${tr(context, 'Обращение гостя:')} ${r['text']}', style: const TextStyle(fontSize: 14)),
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  SolidButton('Принять', onPressed: busy ? null : () => act(() => widget.api.request('/api/pair-requests/${r['id']}', body: {'accept': true}).then((_) {}), ok: 'Устройство принято как пользователь узла.')),
                  const SizedBox(width: 8),
                  SolidButton('Отклонить', tone: SolidTone.danger, onPressed: busy ? null : () => act(() => widget.api.request('/api/pair-requests/${r['id']}', body: {'accept': false}).then((_) {}), ok: 'Запрос отклонён.')),
                ],
              ),
            ),
        ],
      ),
    ),
  );

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      pairBlock(),
      if (loading) const LinearProgressIndicator(),
      if (error != null) UiText(error!),
      if (notice != null) UiText(notice!),
      Row(crossAxisAlignment: CrossAxisAlignment.start, children: [listPane(), detailPane()]),
    ],
  );
}
