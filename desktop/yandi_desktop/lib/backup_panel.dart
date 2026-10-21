import 'dart:async';

import 'package:flutter/material.dart';

import 'preferences.dart';
import 'theme.dart';
import 'transport/api.dart';

/// «Копии связей»: где хранятся копии (сопряжённые телефоны, контакты, группы), последняя копия, «сохранить сейчас». Место выбирается при первой
/// настройке и здесь; узел создаёт в нём папку `yandi-node`, проверяет запись пробным файлом и предупреждает о системном диске.
/// Разделы админские: нужен сеанс администратора.
class BackupPanel extends StatefulWidget {
  final Api api;
  final bool Function() isActive;

  /// Окно после первой настройки: есть «Пропустить», по готовности вызывается [onDone].
  final bool firstRun;
  final VoidCallback? onDone;
  const BackupPanel({super.key, required this.api, required this.isActive, this.firstRun = false, this.onDone});
  @override
  State<BackupPanel> createState() => _BackupPanelState();
}

class _BackupPanelState extends State<BackupPanel> {
  final path = TextEditingController();
  Map<String, dynamic>? status;
  List<Map<String, dynamic>> places = [];
  String? error, note;
  bool busy = false, needsConfirm = false, confirmed = false;
  bool get active => mounted && widget.isActive();

  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    path.dispose();
    super.dispose();
  }

  Future<void> load() async {
    try {
      final st = await widget.api.request('/api/backup/status');
      final pl = await widget.api.request('/api/backup/places');
      if (!mounted) return;
      setState(() {
        status = st;
        places = [for (final x in (pl['places'] is List ? pl['places'] as List : const [])) if (x is Map) x.cast<String, dynamic>()];
        error = null;
      });
    } catch (e) {
      if (mounted) {
        setState(() => error = e is ApiFailure
            ? (e.status == 403 ? 'Нужен сеанс администратора: выйдите из админ-панели и войдите снова.' : e.message)
            : const ApiFailure('protocol').message);
      }
    }
  }

  Future<void> apply(String dir) async {
    if (busy || dir.trim().isEmpty) return;
    setState(() {
      busy = true;
      error = null;
      note = null;
    });
    try {
      final r = await widget.api.request('/api/backup/config', body: {'dir': dir.trim(), 'confirm_system': confirmed});
      if (!mounted) return;
      setState(() {
        status = r;
        needsConfirm = false;
        confirmed = false;
        note = 'Копии связей будут сохраняться в ${r['dir']}';
      });
      widget.onDone?.call();
    } on ApiFailure catch (e) {
      if (!mounted) return;
      setState(() {
        error = e.message;
        needsConfirm = e.kind == 'backup' && e.code == 'system_disk';
      });
    } catch (_) {
      if (mounted) setState(() => error = const ApiFailure('protocol').message);
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> off() async {
    setState(() => busy = true);
    try {
      final r = await widget.api.request('/api/backup/config', body: {'dir': ''});
      if (mounted) setState(() => status = r);
    } catch (e) {
      if (mounted) setState(() => error = e is ApiFailure ? e.message : const ApiFailure('protocol').message);
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> now() async {
    setState(() {
      busy = true;
      error = null;
      note = null;
    });
    try {
      final r = await widget.api.request('/api/backup/export?quiet=1', body: const {});
      if (mounted) setState(() => note = 'Копия сохранена: ${r['saved']}');
      await load();
    } catch (e) {
      if (mounted) setState(() => error = e is ApiFailure ? e.message : const ApiFailure('protocol').message);
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  static String when(Object? ms) {
    final v = (ms is num) ? ms.toInt() : 0;
    if (v == 0) return '—';
    final t = DateTime.fromMillisecondsSinceEpoch(v, isUtc: true);
    String two(int n) => n.toString().padLeft(2, '0');
    return '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)} UTC';
  }

  static String free(Object? b) {
    final v = (b is num) ? b.toInt() : -1;
    if (v < 0) return '?';
    return v >= 1 << 30 ? '${(v / (1 << 30)).toStringAsFixed(1)} GB' : '${v >> 20} MB';
  }

  @override
  Widget build(BuildContext context) {
    final st = status;
    final dir = st?['dir'] is String && (st!['dir'] as String).trim().isNotEmpty ? st['dir'] as String : null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const UiText('Копии связей', style: TextStyle(fontSize: 22)),
        const SizedBox(height: 6),
        const UiText(
          'В копию входят сопряжённые телефоны, контакты и группы (без переписки и файлов). Копия нужна, чтобы после потери диска вернуть их. Сид-фраза возвращает узел и без копии, но телефоны тогда сопрягаются заново.',
        ),
        if (st != null && dir == null) const Padding(padding: EdgeInsets.only(top: 6), child: UiText('Копии не настроены: выберите место.')),
        if (dir != null) ...[
          const SizedBox(height: 6),
          Text('${tr(context, 'Папка копий')}: $dir'),
          Text('${tr(context, 'Последняя копия')}: ${when(st?['last_saved_ms'])} (№${st?['counter'] ?? 0})'),
          UiText('Хранится автокопий: ${st?['keep'] ?? 7}, «сохранить сейчас»: ${st?['keep_manual'] ?? 5}; изменения проверяются каждые 10 минут.'),
          if (st?['last_error'] is String) NoticePanel('${st!['last_error']}', error: true),
        ],
        const SizedBox(height: 10),
        const UiText('Где хранить копии? Узел создаст в выбранном месте папку yandi-node. Храните копии вне системного диска: флешка, другой диск, папка облака.'),
        for (final p in places)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Row(
              children: [
                Expanded(child: Text('${p['path']}  [${p['fs']}, ${tr(context, 'свободно')} ${free(p['free_bytes'])}]${p['writable'] == true ? '' : '  (${tr(context, 'нет прав на запись')})'}')),
                SolidButton('Выбрать', onPressed: busy ? null : () => setState(() => path.text = '${p['path']}'), tone: SolidTone.light),
              ],
            ),
          ),
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 8),
          child: TextField(
            controller: path,
            enabled: !busy,
            enableSuggestions: false,
            autocorrect: false,
            decoration: InputDecoration(labelText: tr(context, 'Место для копий (полный путь, например /media/user/FLASH)'), border: const OutlineInputBorder()),
            onSubmitted: (_) => apply(path.text),
          ),
        ),
        if (needsConfirm)
          CheckboxListTile(
            key: const ValueKey('confirm-system'),
            value: confirmed,
            onChanged: (v) => setState(() => confirmed = v == true),
            controlAffinity: ListTileControlAffinity.leading,
            contentPadding: EdgeInsets.zero,
            title: const UiText('Всё равно сохранять на системный диск'),
          ),
        Wrap(
          children: [
            Padding(padding: const EdgeInsets.only(right: 12), child: SolidButton(busy ? 'Подождите…' : 'Сохранить место', onPressed: busy ? null : () => apply(path.text))),
            if (dir != null) Padding(padding: const EdgeInsets.only(right: 12), child: SolidButton('Сохранить копию сейчас', onPressed: busy ? null : now, tone: SolidTone.light)),
            if (dir != null) Padding(padding: const EdgeInsets.only(right: 12), child: SolidButton('Выключить копии', onPressed: busy ? null : off, tone: SolidTone.light)),
            if (widget.firstRun)
              SolidButton('Пропустить (напомню позже)', onPressed: busy ? null : widget.onDone, tone: SolidTone.light),
          ],
        ),
        if (note != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(tr(context, note!))),
        if (error != null) Padding(padding: const EdgeInsets.only(top: 8), child: NoticePanel(tr(context, error!), error: true)),
      ],
    );
  }
}
