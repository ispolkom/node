import 'dart:async';
import 'dart:io';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../services/files_host.dart';
import '../services/file_transfer_service.dart';
import '../services/vault_files.dart';
import '../theme.dart';
import 'file_viewer_screen.dart';

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
      v = VaultFiles(await FileTransferService.downloadsDir(), wipe: AppState.wipeFile);
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

  /// «+» на вкладке: взять файл из памяти телефона и положить в хранилище (чтобы переслать оттуда). Временную копию выбора сразу чистим.
  Future<void> addFiles() async {
    final res = await FilePicker.platform.pickFiles(allowMultiple: true);
    if (res == null) return;
    final v = _vault;
    if (v == null) return;
    for (final f in res.files) {
      final p = f.path;
      if (p != null) await v.importFile(p, name: f.name);
    }
    try { await FilePicker.platform.clearTemporaryFiles(); } catch (_) {}
    await reload();
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
        title: Text(n == 1 ? 'Удалить файл?' : 'Удалить файлы ($n)?', style: const TextStyle(color: AppTheme.text)),
        content: Text('$what будет удалено сразу и насовсем, без корзины.', style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Отмена')),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('Удалить', style: TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return ok == true;
  }

  Future<void> _delete(List<VaultFile> files) async {
    if (files.isEmpty || !await _confirmDelete(files.length, files.length == 1 ? '«${files.first.name}»' : 'Выбранное')) return;
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
      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Некому отправлять: в списке нет контактов')));
      return;
    }
    final picked = await showModalBottomSheet<String>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: ListView(shrinkWrap: true, children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
            child: Text(files.length == 1 ? 'Переслать «${files.first.name}»' : 'Переслать файлы (${files.length})', style: const TextStyle(color: AppTheme.text, fontSize: 16)),
          ),
          const Padding(
            padding: EdgeInsets.fromLTRB(16, 0, 16, 8),
            child: Text('Только внутри сети YANDI: файл уходит зашифрованным выбранному контакту.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
          for (final c in contacts)
            ListTile(
              leading: Icon(Icons.person_outline, color: c.online ? AppTheme.accent : AppTheme.textSecondary),
              title: Text(c.displayName, style: const TextStyle(color: AppTheme.text)),
              subtitle: Text(c.online ? 'в сети' : 'не в сети', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onTap: () => Navigator.pop(ctx, c.peerId),
            ),
        ]),
      ),
    );
    if (picked == null || !mounted) return;
    final name = contacts.firstWhere((c) => c.peerId == picked).displayName;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('Отправка → $name. Ход — в чате с ним.')));
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
        child: Image.file(File(f.path), width: 48, height: 48, fit: BoxFit.cover, cacheWidth: 96, errorBuilder: (_, __, ___) => Icon(_icon(f), color: AppTheme.textSecondary, size: 32)),
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
    if (_loading) return const Center(child: CircularProgressIndicator(color: AppTheme.accent));
    final fmt = DateFormat('dd.MM.yy HH:mm');
    final selected = _files.where((f) => _selected.contains(f.path)).toList();
    return Column(children: [
      Expanded(
        child: _files.isEmpty
            ? ListView(children: const [
                SizedBox(height: 120),
                Icon(Icons.folder_open_outlined, color: AppTheme.textSecondary, size: 64),
                SizedBox(height: 16),
                Center(child: Text('Файлов пока нет', style: TextStyle(color: AppTheme.textSecondary))),
                Padding(
                  padding: EdgeInsets.all(24),
                  child: Text('Здесь лежит всё, что вы получили, и то, что добавили кнопкой «+». Хранилище изолировано: другие приложения его не видят.',
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
                              icon: const Icon(Icons.more_vert, color: AppTheme.textSecondary),
                              onSelected: (v) {
                                if (v == 'open') _open(f);
                                if (v == 'share') _share([f]);
                                if (v == 'delete') _delete([f]);
                              },
                              itemBuilder: (_) => const [
                                PopupMenuItem(value: 'open', child: Text('Открыть')),
                                PopupMenuItem(value: 'share', child: Text('Переслать контакту')),
                                PopupMenuItem(value: 'delete', child: Text('Удалить', style: TextStyle(color: Colors.redAccent))),
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
              IconButton(tooltip: 'Снять выбор', icon: const Icon(Icons.close), onPressed: () => setState(_selected.clear)),
              Text('Выбрано: ${selected.length}', style: const TextStyle(color: AppTheme.text)),
              const Spacer(),
              TextButton.icon(onPressed: () => _share(selected), icon: const Icon(Icons.share_outlined), label: const Text('Переслать')),
              TextButton.icon(onPressed: () => _delete(selected), icon: const Icon(Icons.delete_outline, color: Colors.redAccent), label: const Text('Удалить', style: TextStyle(color: Colors.redAccent))),
            ]),
          ),
        ),
    ]);
  }
}

/// Подпись размера (общая для списка и просмотра).
class FileViewerScreenSize {
  static String human(int b) => b < 1024 ? '$b Б' : b < 1024 * 1024 ? '${(b / 1024).toStringAsFixed(1)} КБ' : '${(b / 1024 / 1024).toStringAsFixed(1)} МБ';
}
