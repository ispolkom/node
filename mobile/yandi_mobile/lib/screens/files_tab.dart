import 'dart:async';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../services/files_host.dart';
import '../services/vault_files.dart';
import '../theme.dart';
import '../widgets/vault_image.dart';
import 'file_viewer_screen.dart';
import '../l10n.dart';

/// Вкладка «Файлы»: общее защищённое хранилище приложения. Смотреть можно, не выходя из приложения; удаление — сразу и насовсем, без корзины;
/// пересылать можно только контактам YANDI (системного «Поделиться» нет: файл не должен уйти к постороннему приложению).
class FilesTab extends StatefulWidget {
  /// Для тестов: готовое хранилище вместо папки приложения.
  final VaultFiles? vault;
  final FilesHost? host;
  const FilesTab({super.key, this.vault, this.host});
  @override
  State<FilesTab> createState() => FilesTabState();
}

class FilesTabState extends State<FilesTab> {
  VaultFiles? _vault;
  List<VaultFile> _files = [];
  final Set<String> _selected = {};
  bool _loading = true;
  int _seenStates = -1;

  @override
  void initState() {
    super.initState();
    reload();
  }

  Future<void> reload() async {
    var v = _vault ?? widget.vault;
    if (v == null) {
      if (!mounted) return;
      v = await context.read<AppState>().vaultFiles();
    }
    _vault = v;
    unawaited(v.finishPendingWipes());   // доделать затирание, прерванное закрытием приложения
    final list = await v.list();
    if (!mounted) return;
    setState(() {
      _files = list;
      _loading = false;
      _selected.removeWhere((p) => !list.any((f) => f.path == p));
    });
  }

  Future<void> _open(VaultFile f) async {
    await Navigator.push(context, MaterialPageRoute(
      builder: (_) => FileViewerScreen(file: f, onShare: () => _share([f]), onDelete: () async { await _vault?.deleteForever(f.path); }),
    ));
    await reload();
  }

  Future<bool> _confirmDelete(int n, String what) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(n == 1 ? tr('Удалить файл?') : trf('Удалить файлы ({n})?', {'n': n}), style: const TextStyle(color: AppTheme.text)),
        content: Text(trf('{what} будет удалено сразу и насовсем, без корзины.', {'what': what}), style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'))),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Удалить'), style: TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return ok == true;
  }

  Future<void> _delete(List<VaultFile> files) async {
    if (files.isEmpty || !await _confirmDelete(files.length, files.length == 1 ? '«${files.first.name}»' : tr('Выбранное'))) return;
    await _vault?.deleteAll(files.map((f) => f.path));
    _selected.clear();
    await reload();
  }

  /// Выбор контакта YANDI и отправка: ход виден в чате с ним.
  Future<void> _share(List<VaultFile> files) async {
    final FilesHost state = widget.host ?? context.read<AppState>();
    // предлагаем сохранённые контакты и устройства, которые на главном экране (давно молчащие скрыты), контакты — первыми, в сети — выше
    final now = DateTime.now().millisecondsSinceEpoch;
    final contacts = state.contacts.where((c) => c.saved || !c.isStale(now)).toList()
      ..sort((a, b) => a.saved != b.saved ? (a.saved ? -1 : 1) : a.online != b.online ? (a.online ? -1 : 1) : a.displayName.compareTo(b.displayName));
    if (contacts.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Некому отправлять: в списке нет контактов'))));
      return;
    }
    final picked = await showModalBottomSheet<String>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: ListView(shrinkWrap: true, children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
            child: Text(files.length == 1 ? trf('Переслать «{n}»', {'n': files.first.name}) : trf('Переслать файлы ({n})', {'n': files.length}), style: const TextStyle(color: AppTheme.text, fontSize: 16)),
          ),
          Padding(
            padding: EdgeInsets.fromLTRB(16, 0, 16, 8),
            child: Text(tr('Только внутри сети YANDI: файл уходит зашифрованным выбранному контакту.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
          for (final c in contacts)
            ListTile(
              leading: Icon(Icons.person_outline, color: c.online ? AppTheme.accent : AppTheme.textSecondary),
              title: Text(c.displayName, style: const TextStyle(color: AppTheme.text)),
              subtitle: Text(c.online ? tr('в сети') : tr('не в сети'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onTap: () => Navigator.pop(ctx, c.peerId),
            ),
        ]),
      ),
    );
    if (picked == null || !mounted) return;
    final name = contacts.firstWhere((c) => c.peerId == picked).displayName;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(trf('Отправка → {name}. Ход — в чате с ним.', {'name': name}))));
    for (final f in files) {
      state.sendFile(picked, f.path, name: f.name).then((err) {
        if (err != null && mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('${f.name}: $err')));
      });
    }
    setState(() => _selected.clear());
  }

  IconData _icon(VaultFile f) {
    if (f.isImage) return Icons.image_outlined;
    if (f.isVideo) return Icons.movie_outlined;
    if (f.isAudio) return Icons.audiotrack_outlined;
    if (f.mime == 'application/pdf') return Icons.picture_as_pdf_outlined;
    if (f.mime == 'application/zip') return Icons.folder_zip_outlined;
    return Icons.insert_drive_file_outlined;
  }

  Widget _leading(VaultFile f) {
    final selected = _selected.contains(f.path);
    if (selected) return const CircleAvatar(backgroundColor: AppTheme.accent, child: Icon(Icons.check, color: Colors.black));
    if (f.isImage) {
      return ClipRRect(
        borderRadius: BorderRadius.circular(8),
        child: VaultImage(path: f.path, width: 48, height: 48, cacheWidth: 96, fallback: (_) => Icon(_icon(f), color: AppTheme.textSecondary, size: 32)),
      );
    }
    return Icon(_icon(f), color: AppTheme.textSecondary, size: 32);
  }

  @override
  Widget build(BuildContext context) {
    // скачанное в чате появляется здесь само: список перечитывается, когда меняется набор передач
    final FilesHost host = widget.host ?? context.watch<AppState>();
    final states = host.transferCount;
    if (states != _seenStates) {
      final first = _seenStates == -1;
      _seenStates = states;
      if (!first) WidgetsBinding.instance.addPostFrameCallback((_) => reload());
    }
    if (_loading) return Center(child: CircularProgressIndicator(color: AppTheme.accent));
    final fmt = DateFormat('dd.MM.yy HH:mm');
    final selected = _files.where((f) => _selected.contains(f.path)).toList();
    return Column(children: [
      Expanded(
        child: _files.isEmpty
            ? ListView(children: [
                SizedBox(height: 120),
                Icon(Icons.folder_open_outlined, color: AppTheme.textSecondary, size: 64),
                SizedBox(height: 16),
                Center(child: Text(tr('Файлов пока нет'), style: TextStyle(color: AppTheme.textSecondary))),
                Padding(
                  padding: EdgeInsets.all(24),
                  child: Text(tr('Здесь лежит всё, что вы получили в чатах и группах. Добавить файл отсюда нельзя: отправляйте файлы из чата или группы. Хранилище изолировано: другие приложения его не видят.'),
                      textAlign: TextAlign.center, style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                ),
              ])
            : RefreshIndicator(
                color: AppTheme.accent,
                backgroundColor: AppTheme.surface,
                onRefresh: reload,
                child: ListView.separated(
                  physics: const AlwaysScrollableScrollPhysics(),
                  itemCount: _files.length,
                  separatorBuilder: (_, __) => const Divider(height: 1, color: AppTheme.divider),
                  itemBuilder: (ctx, i) {
                    final f = _files[i];
                    final isSel = _selected.contains(f.path);
                    return ListTile(
                      selected: isSel,
                      selectedTileColor: AppTheme.accent.withOpacity(0.12),
                      leading: _leading(f),
                      title: Text(f.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text)),
                      subtitle: Text('${FileViewerScreenSize.human(f.size)} · ${fmt.format(f.modified)}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      onTap: () {
                        if (_selected.isNotEmpty) {
                          setState(() => isSel ? _selected.remove(f.path) : _selected.add(f.path));
                        } else {
                          _open(f);
                        }
                      },
                      onLongPress: () => setState(() => isSel ? _selected.remove(f.path) : _selected.add(f.path)),
                      trailing: _selected.isNotEmpty
                          ? null
                          : PopupMenuButton<String>(
                              color: AppTheme.surface,
                              icon: Icon(Icons.more_vert, color: AppTheme.textSecondary),
                              onSelected: (v) {
                                if (v == 'open') _open(f);
                                if (v == 'share') _share([f]);
                                if (v == 'delete') _delete([f]);
                              },
                              itemBuilder: (_) => [
                                PopupMenuItem(value: 'open', child: Text(tr('Открыть'))),
                                PopupMenuItem(value: 'share', child: Text(tr('Переслать контакту'))),
                                PopupMenuItem(value: 'delete', child: Text(tr('Удалить'), style: TextStyle(color: Colors.redAccent))),
                              ],
                            ),
                    );
                  },
                ),
              ),
      ),
      if (selected.isNotEmpty)
        Material(
          color: AppTheme.surface,
          child: SafeArea(
            top: false,
            child: Row(children: [
              IconButton(tooltip: tr('Снять выбор'), icon: const Icon(Icons.close), onPressed: () => setState(_selected.clear)),
              Text(trf('Выбрано: {n}', {'n': selected.length}), style: const TextStyle(color: AppTheme.text)),
              const Spacer(),
              TextButton.icon(onPressed: () => _share(selected), icon: const Icon(Icons.share_outlined), label: Text(tr('Переслать'))),
              TextButton.icon(onPressed: () => _delete(selected), icon: const Icon(Icons.delete_outline, color: Colors.redAccent), label: Text(tr('Удалить'), style: TextStyle(color: Colors.redAccent))),
            ]),
          ),
        ),
    ]);
  }
}

/// Подпись размера (общая для списка и просмотра).
class FileViewerScreenSize {
  static String human(int b) => b < 1024 ? trf('{n} Б', {'n': b}) : b < 1024 * 1024 ? trf('{n} КБ', {'n': (b / 1024).toStringAsFixed(1)}) : trf('{n} МБ', {'n': (b / 1024 / 1024).toStringAsFixed(1)});
}
