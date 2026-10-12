import 'dart:io';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../models/file_offer.dart';
import '../services/app_state.dart';
import '../services/file_transfer_service.dart';
import '../services/files_host.dart';
import '../services/vault_files.dart';
import '../theme.dart';
import '../widgets/recipients_sheet.dart';
import 'file_viewer_screen.dart';

/// Вид файла для раздела папки.
enum FileKind { documents, music, video, pictures, archives, other }

const _kindTitles = {
  FileKind.documents: 'Документы',
  FileKind.music: 'Музыка',
  FileKind.video: 'Видео',
  FileKind.pictures: 'Картинки',
  FileKind.archives: 'Архивы',
  FileKind.other: 'Прочее',
};

FileKind fileKindOf(String name, String mime) {
  final ext = name.contains('.') ? name.split('.').last.toLowerCase() : '';
  if (mime.startsWith('image/')) return FileKind.pictures;
  if (mime.startsWith('video/')) return FileKind.video;
  if (mime.startsWith('audio/')) return FileKind.music;
  if (const ['zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'bz2', 'xz'].contains(ext) ||
      mime == 'application/zip' ||
      mime == 'application/x-rar-compressed' ||
      mime == 'application/gzip') {
    return FileKind.archives;
  }
  if (mime == 'application/pdf' ||
      mime.startsWith('text/') ||
      const ['pdf', 'txt', 'md', 'doc', 'docx', 'odt', 'rtf', 'xls', 'xlsx', 'ods', 'csv', 'ppt', 'pptx', 'odp', 'epub', 'fb2']
          .contains(ext)) {
    return FileKind.documents;
  }
  return FileKind.other;
}

/// «Файлы чата»: отдельная папка внутри защищённого пространства — только файлы этого собеседника (и присланные, и отправленные).
/// Папка называется коротким идентификатором контакта. Файлы можно открыть и посмотреть или послушать внутри приложения,
/// удалять по одному, выбранными или все сразу (с подтверждением). Наружу из этой папки файлы не выносятся.
class ChatFilesScreen extends StatefulWidget {
  final String title;
  final String shortId;
  final List<FileOffer> offers;
  /// Для тестов: готовое хранилище вместо папки приложения.
  final VaultFiles? vault;
  /// Для тестов: хозяин передач (контакты и отправка) вместо AppState.
  final FilesHost? host;
  /// Предел получателей на один файл за сеанс (по умолчанию 20; параметр — для тестов).
  final int maxPerFile;
  const ChatFilesScreen({super.key, required this.title, required this.shortId, required this.offers, this.vault, this.host, this.maxPerFile = defaultMaxPerFile});

  /// За один раз — не больше стольких получателей; суммарно на один файл — не больше defaultMaxPerFile.
  static const maxPerBatch = 5;
  static const defaultMaxPerFile = 20;

  @override
  State<ChatFilesScreen> createState() => ChatFilesScreenState();
}

class ChatFilesScreenState extends State<ChatFilesScreen> {
  VaultFiles? _vault;
  List<VaultFile> _files = [];
  final Set<String> _selected = {};
  /// Кому уже пересылали файл в этом сеансе (путь файла → получатели): предел на один файл.
  final Map<String, Set<String>> _sentTo = {};
  bool _loading = true;

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
    final out = <VaultFile>[];
    final seen = <String>{};
    for (final o in widget.offers) {
      if (!seen.add(o.tid)) continue;
      final p = '${v.dir.path}/${FileTransferService.localName(o)}';
      if (!await File(p).exists()) continue;
      try {
        out.add(await v.describe(p));
      } catch (_) {
        // повреждённый файл в папке не показываем
      }
    }
    out.sort((a, b) => b.modified.compareTo(a.modified));
    if (!mounted) return;
    setState(() {
      _files = out;
      _loading = false;
      _selected.removeWhere((p) => !out.any((f) => f.path == p));
    });
  }

  static String _size(int b) {
    if (b < 1024) return '$b Б';
    if (b < 1024 * 1024) return '${(b / 1024).toStringAsFixed(1)} КБ';
    if (b < 1024 * 1024 * 1024) return '${(b / 1024 / 1024).toStringAsFixed(1)} МБ';
    return '${(b / 1024 / 1024 / 1024).toStringAsFixed(2)} ГБ';
  }

  IconData _icon(FileKind k) => switch (k) {
        FileKind.documents => Icons.description_outlined,
        FileKind.music => Icons.audiotrack_outlined,
        FileKind.video => Icons.movie_outlined,
        FileKind.pictures => Icons.image_outlined,
        FileKind.archives => Icons.folder_zip_outlined,
        FileKind.other => Icons.insert_drive_file_outlined,
      };

  Future<bool> _confirm(String title, String text, {String yes = 'Да', String no = 'Нет'}) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(title, style: const TextStyle(color: AppTheme.text)),
        content: Text(text, style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(no)),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(yes, style: const TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return ok == true;
  }

  Future<void> _deleteSelected() async {
    final files = _files.where((f) => _selected.contains(f.path)).toList();
    if (files.isEmpty) return;
    final n = files.length;
    if (!await _confirm(n == 1 ? 'Удалить файл?' : 'Удалить файлы ($n)?',
        'Будет удалено сразу и насовсем, без корзины.')) {
      return;
    }
    await _vault?.deleteAll(files.map((f) => f.path));
    _selected.clear();
    await reload();
  }

  Future<void> _clearFolder() async {
    if (_files.isEmpty) return;
    if (!await _confirm('Уверены?', 'Все файлы этого чата (${_files.length}) будут удалены сразу и насовсем, без корзины.')) return;
    await _vault?.deleteAll(_files.map((f) => f.path).toList());
    _selected.clear();
    await reload();
  }

  Future<void> _trashMenu() async {
    final size = MediaQuery.of(context).size;
    final choice = await showMenu<String>(
      context: context,
      color: AppTheme.surface,
      position: RelativeRect.fromLTRB(size.width, kToolbarHeight + MediaQuery.of(context).padding.top, 0, 0),
      items: const [
        PopupMenuItem(value: 'clear', child: Text('Очистить папку', style: TextStyle(color: Colors.redAccent))),
        PopupMenuItem(value: 'cancel', child: Text('Отмена', style: TextStyle(color: AppTheme.text))),
      ],
    );
    if (choice == 'clear') await _clearFolder();
  }

  Future<void> _forward(List<VaultFile> files) async {
    if (files.isEmpty) return;
    final FilesHost host = widget.host ?? context.read<AppState>();
    final contacts = forwardCandidates(host.contacts);
    if (contacts.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Некому отправлять: в списке нет контактов')));
      return;
    }
    final to = await pickRecipients(context,
        title: files.length == 1 ? 'Переслать «${files.first.name}»' : 'Переслать файлы (${files.length})', contacts: contacts);
    if (to == null || to.isEmpty || !mounted) return;
    // предел на файл: всего не больше 20 разных получателей
    for (final f in files) {
      final all = {...?_sentTo[f.path], ...to.map((c) => c.peerId)};
      if (all.length > widget.maxPerFile) {
        ScaffoldMessenger.of(context).clearSnackBars();
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(
            content: Text('«${f.name}»: не больше ${widget.maxPerFile} получателей на файл (уже ${_sentTo[f.path]?.length ?? 0})')));
        return;
      }
    }
    final names = to.map((c) => c.displayName).join(', ');
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('Отправка → $names. Ход — в чатах с ними.')));
    for (final f in files) {
      _sentTo.putIfAbsent(f.path, () => {}).addAll(to.map((c) => c.peerId));
      for (final c in to) {
        host.sendFile(c.peerId, f.path, name: f.name).then((err) {
          if (err != null && mounted) {
            ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('${f.name} → ${c.displayName}: $err')));
          }
        });
      }
    }
    setState(_selected.clear);
  }

  /// Удержание файла: Переслать, Выбрать (несколько), Удалить.
  Future<void> _fileMenu(VaultFile f, Offset at) async {
    final size = MediaQuery.of(context).size;
    final choice = await showMenu<String>(
      context: context,
      color: AppTheme.surface,
      position: RelativeRect.fromLTRB(at.dx, at.dy, size.width - at.dx, size.height - at.dy),
      items: const [
        PopupMenuItem(value: 'forward', height: 44, child: Text('Переслать', style: TextStyle(color: AppTheme.text, fontSize: 15))),
        PopupMenuItem(value: 'select', height: 44, child: Text('Выбрать', style: TextStyle(color: AppTheme.text, fontSize: 15))),
        PopupMenuItem(value: 'delete', height: 44, child: Text('Удалить', style: TextStyle(color: Colors.redAccent, fontSize: 15))),
      ],
    );
    if (!mounted) return;
    switch (choice) {
      case 'forward':
        await _forward([f]);
      case 'select':
        setState(() => _selected.add(f.path));
      case 'delete':
        setState(() => _selected..clear()..add(f.path));
        await _deleteSelected();
        // отказ в подтверждении не должен оставлять файл выбранным
        if (mounted) setState(_selected.clear);
    }
  }

  Future<void> _open(VaultFile f) async {
    await Navigator.push(context, MaterialPageRoute(
      builder: (_) => FileViewerScreen(file: f, onDelete: () async { await _vault?.deleteForever(f.path); }),
    ));
    await reload();
  }

  @override
  Widget build(BuildContext context) {
    final selecting = _selected.isNotEmpty;
    final groups = <FileKind, List<VaultFile>>{};
    for (final f in _files) {
      groups.putIfAbsent(fileKindOf(f.name, f.mime), () => []).add(f);
    }
    return Scaffold(
      appBar: AppBar(
        leading: selecting
            ? IconButton(icon: const Icon(Icons.close), tooltip: 'Снять выбор', onPressed: () => setState(_selected.clear))
            : null,
        title: selecting
            ? Text('Выбрано: ${_selected.length}', style: const TextStyle(fontSize: 16))
            : Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                const Text('Файлы чата', style: TextStyle(fontSize: 16)),
                Text('${widget.title} · ${widget.shortId}',
                    overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 11, color: AppTheme.textSecondary)),
              ]),
        actions: [
          if (selecting)
            IconButton(
              key: const ValueKey('files-forward-selected'),
              tooltip: 'Переслать выбранные',
              icon: const Icon(Icons.forward_to_inbox_outlined),
              onPressed: () => _forward(_files.where((f) => _selected.contains(f.path)).toList()),
            ),
          if (selecting)
            IconButton(
              key: const ValueKey('files-delete-selected'),
              tooltip: 'Удалить выбранные',
              icon: const Icon(Icons.delete_outline, color: Colors.redAccent),
              onPressed: _deleteSelected,
            )
          else
            IconButton(
              key: const ValueKey('files-trash'),
              tooltip: 'Корзина',
              icon: const Icon(Icons.delete_outline),
              onPressed: _trashMenu,
            ),
        ],
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : _files.isEmpty
              ? const Center(
                  child: Padding(
                    padding: EdgeInsets.all(32),
                    child: Text('В этой папке пока нет файлов. Файлы, которые вы получаете и отправляете в этом чате, появятся здесь.',
                        textAlign: TextAlign.center, style: TextStyle(color: AppTheme.textSecondary)),
                  ),
                )
              : ListView(
                  children: [
                    for (final kind in FileKind.values)
                      if (groups[kind] != null) ...[
                        Padding(
                          padding: const EdgeInsets.fromLTRB(16, 14, 16, 4),
                          child: Text('${_kindTitles[kind]} · ${groups[kind]!.length}',
                              style: const TextStyle(color: AppTheme.accent, fontSize: 13, fontWeight: FontWeight.bold)),
                        ),
                        for (final f in groups[kind]!)
                          GestureDetector(
                            key: ValueKey('file-${f.id}'),
                            onLongPressStart: (d) {
                              if (selecting) {
                                setState(() => _selected.contains(f.path) ? _selected.remove(f.path) : _selected.add(f.path));
                              } else {
                                _fileMenu(f, d.globalPosition);
                              }
                            },
                            child: ListTile(
                              selected: _selected.contains(f.path),
                              selectedTileColor: AppTheme.accent.withValues(alpha: 0.15),
                              leading: Icon(_selected.contains(f.path) ? Icons.check_circle : _icon(kind),
                                  color: _selected.contains(f.path) ? AppTheme.accent : AppTheme.textSecondary),
                              title: Text(f.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text)),
                              subtitle: Text('${_size(f.size)} · ${DateFormat('dd.MM.yyyy HH:mm').format(f.modified)}',
                                  style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                              onTap: () {
                                if (selecting) {
                                  setState(() => _selected.contains(f.path) ? _selected.remove(f.path) : _selected.add(f.path));
                                } else {
                                  _open(f);
                                }
                              },
                            ),
                          ),
                      ],
                  ],
                ),
    );
  }
}
