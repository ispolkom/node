import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
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
import '../l10n.dart';

/// Вид файла для раздела папки.
enum FileKind { documents, music, video, pictures, archives, other }

Map<FileKind, String> get _kindTitles => {
  FileKind.documents: tr('Документы'),
  FileKind.music: tr('Музыка'),
  FileKind.video: tr('Видео'),
  FileKind.pictures: tr('Картинки'),
  FileKind.archives: tr('Архивы'),
  FileKind.other: tr('Прочее'),
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

/// Ссылка из переписки для раздела «Ссылки» папки: адрес и когда он появился.
class FileLink {
  final String url;
  final DateTime when;
  const FileLink(this.url, this.when);

  static final _re = RegExp(r'(https?://|www\.)[^\s<>"]+', caseSensitive: false);

  /// Все адреса из текста (без хвостовой пунктуации).
  static List<FileLink> inText(String text, DateTime when) => [
        for (final m in _re.allMatches(text)) FileLink(m.group(0)!.replaceAll(RegExp(r'[.,;:!?)\]]+$'), ''), when),
      ];
}

/// Разделы-вкладки папки (как в мессенджерах): вид содержимого, без сообщений.
List<String> get _tabs => [tr('Все'), tr('Картинки'), tr('Файлы'), tr('Аудио'), tr('Видео'), tr('Ссылки')];

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
  /// Подсказка пустой папки (для группы своя).
  final String? emptyHint;
  /// Ссылки из переписки (раздел «Ссылки»); сами сообщения в папке не показываются.
  final List<FileLink> links;
  /// Скачать файл, которого ещё нет в закрытой среде (для групп): такие файлы показываются в папке с пометкой «не скачан».
  final Future<bool> Function(FileOffer)? onFetch;
  const ChatFilesScreen({super.key, required this.title, required this.shortId, required this.offers, this.vault, this.host, this.maxPerFile = defaultMaxPerFile, this.emptyHint, this.links = const [], this.onFetch});

  /// За один раз — не больше стольких получателей; суммарно на один файл — не больше defaultMaxPerFile.
  static const maxPerBatch = 5;
  static const defaultMaxPerFile = 20;

  @override
  State<ChatFilesScreen> createState() => ChatFilesScreenState();
}

class ChatFilesScreenState extends State<ChatFilesScreen> {
  VaultFiles? _vault;
  List<VaultFile> _files = [];
  List<FileOffer> _missing = [];
  final Set<String> _fetching = {};
  final Set<String> _selected = {};
  /// Кому уже пересылали файл в этом сеансе (путь файла → получатели): предел на один файл.
  final Map<String, Set<String>> _sentTo = {};
  bool _loading = true;
  int _tab = 0;

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
    final missing = <FileOffer>[];
    for (final o in widget.offers) {
      if (!seen.add(o.tid)) continue;
      final p = '${v.dir.path}/${FileTransferService.localName(o)}';
      if (!await File(p).exists()) {
        if (widget.onFetch != null) missing.add(o);
        continue;
      }
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
      _missing = missing;
      _loading = false;
      _selected.removeWhere((p) => !out.any((f) => f.path == p));
    });
  }

  static String _size(int b) {
    if (b < 1024) return trf('{n} Б', {'n': b});
    if (b < 1024 * 1024) return trf('{n} КБ', {'n': (b / 1024).toStringAsFixed(1)});
    if (b < 1024 * 1024 * 1024) return trf('{n} МБ', {'n': (b / 1024 / 1024).toStringAsFixed(1)});
    return trf('{n} ГБ', {'n': (b / 1024 / 1024 / 1024).toStringAsFixed(2)});
  }

  IconData _icon(FileKind k) => switch (k) {
        FileKind.documents => Icons.description_outlined,
        FileKind.music => Icons.audiotrack_outlined,
        FileKind.video => Icons.movie_outlined,
        FileKind.pictures => Icons.image_outlined,
        FileKind.archives => Icons.folder_zip_outlined,
        FileKind.other => Icons.insert_drive_file_outlined,
      };

  Future<bool> _confirm(String title, String text, {String? yes, String? no}) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(title, style: const TextStyle(color: AppTheme.text)),
        content: Text(text, style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(no ?? tr('Нет'))),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(yes ?? tr('Да'), style: const TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return ok == true;
  }

  Future<void> _deleteSelected() async {
    final files = _files.where((f) => _selected.contains(f.path)).toList();
    if (files.isEmpty) return;
    final n = files.length;
    if (!await _confirm(n == 1 ? tr('Удалить файл?') : trf('Удалить файлы ({n})?', {'n': n}),
        tr('Будет удалено сразу и насовсем, без корзины.'))) {
      return;
    }
    await _vault?.deleteAll(files.map((f) => f.path));
    _selected.clear();
    await reload();
  }

  Future<void> _clearFolder() async {
    if (_files.isEmpty) return;
    if (!await _confirm(tr('Уверены?'), trf('Все файлы этого чата ({n}) будут удалены сразу и насовсем, без корзины.', {'n': _files.length}))) return;
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
      items: [
        PopupMenuItem(value: 'clear', child: Text(tr('Очистить папку'), style: TextStyle(color: Colors.redAccent))),
        PopupMenuItem(value: 'cancel', child: Text(tr('Отмена'), style: TextStyle(color: AppTheme.text))),
      ],
    );
    if (choice == 'clear') await _clearFolder();
  }

  Future<void> _forward(List<VaultFile> files) async {
    if (files.isEmpty) return;
    final FilesHost host = widget.host ?? context.read<AppState>();
    final contacts = forwardCandidates(host.contacts);
    if (contacts.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Некому отправлять: в списке нет контактов'))));
      return;
    }
    final to = await pickRecipients(context,
        title: files.length == 1 ? trf('Переслать «{n}»', {'n': files.first.name}) : trf('Переслать файлы ({n})', {'n': files.length}), contacts: contacts);
    if (to == null || to.isEmpty || !mounted) return;
    // предел на файл: всего не больше 20 разных получателей
    for (final f in files) {
      final all = {...?_sentTo[f.path], ...to.map((c) => c.peerId)};
      if (all.length > widget.maxPerFile) {
        ScaffoldMessenger.of(context).clearSnackBars();
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(
            content: Text(trf('«{name}»: не больше {max} получателей на файл (уже {n})', {'name': f.name, 'max': widget.maxPerFile, 'n': _sentTo[f.path]?.length ?? 0}))));
        return;
      }
    }
    final names = to.map((c) => c.displayName).join(', ');
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(trf('Отправка → {names}. Ход — в чатах с ними.', {'names': names}))));
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
      items: [
        PopupMenuItem(value: 'forward', height: 44, child: Text(tr('Переслать'), style: TextStyle(color: AppTheme.text, fontSize: 15))),
        PopupMenuItem(value: 'select', height: 44, child: Text(tr('Выбрать'), style: TextStyle(color: AppTheme.text, fontSize: 15))),
        PopupMenuItem(value: 'delete', height: 44, child: Text(tr('Удалить'), style: TextStyle(color: Colors.redAccent, fontSize: 15))),
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
    const tabKinds = <int, Set<FileKind>>{
      1: {FileKind.pictures},
      2: {FileKind.documents, FileKind.archives, FileKind.other},
      3: {FileKind.music},
      4: {FileKind.video},
    };
    for (final f in _files) {
      final k = fileKindOf(f.name, f.mime);
      if (_tab == 0 || (tabKinds[_tab]?.contains(k) ?? false)) groups.putIfAbsent(k, () => []).add(f);
    }
    final showLinks = (_tab == 0 || _tab == 5) && widget.links.isNotEmpty;
    final missingBy = <FileKind, List<FileOffer>>{};
    for (final o in _missing) {
      final k = fileKindOf(o.name, o.mime);
      if (_tab == 0 || (tabKinds[_tab]?.contains(k) ?? false)) missingBy.putIfAbsent(k, () => []).add(o);
    }
    return Scaffold(
      appBar: AppBar(
        leading: selecting
            ? IconButton(icon: const Icon(Icons.close), tooltip: tr('Снять выбор'), onPressed: () => setState(_selected.clear))
            : null,
        title: selecting
            ? Text(trf('Выбрано: {n}', {'n': _selected.length}), style: const TextStyle(fontSize: 16))
            : Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(tr('Файлы чата'), style: TextStyle(fontSize: 16)),
                Text('${widget.title} · ${widget.shortId}',
                    overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 11, color: AppTheme.textSecondary)),
              ]),
        actions: [
          if (selecting)
            IconButton(
              key: const ValueKey('files-forward-selected'),
              tooltip: tr('Переслать выбранные'),
              icon: const Icon(Icons.forward_to_inbox_outlined),
              onPressed: () => _forward(_files.where((f) => _selected.contains(f.path)).toList()),
            ),
          if (selecting)
            IconButton(
              key: const ValueKey('files-delete-selected'),
              tooltip: tr('Удалить выбранные'),
              icon: const Icon(Icons.delete_outline, color: Colors.redAccent),
              onPressed: _deleteSelected,
            )
          else
            IconButton(
              key: const ValueKey('files-trash'),
              tooltip: tr('Корзина'),
              icon: const Icon(Icons.delete_outline),
              onPressed: _trashMenu,
            ),
        ],
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : _files.isEmpty && _missing.isEmpty && widget.links.isEmpty
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(32),
                    child: Text(widget.emptyHint ?? tr('В этой папке пока нет файлов. Файлы, которые вы получаете и отправляете в этом чате, появятся здесь.'),
                        textAlign: TextAlign.center, style: const TextStyle(color: AppTheme.textSecondary)),
                  ),
                )
              : ListView(
                  children: [
                    SizedBox(
                      height: 44,
                      child: ListView(
                        key: const ValueKey('files-tabs'),
                        scrollDirection: Axis.horizontal,
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        children: [
                          for (var i = 0; i < _tabs.length; i++)
                            Padding(
                              padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
                              child: ChoiceChip(
                                label: Text(_tabs[i], style: TextStyle(fontSize: 12, color: _tab == i ? Colors.black : AppTheme.text)),
                                selected: _tab == i,
                                selectedColor: AppTheme.accent,
                                backgroundColor: AppTheme.surface,
                                onSelected: (_) => setState(() => _tab = i),
                              ),
                            ),
                        ],
                      ),
                    ),
                    if (groups.isEmpty && missingBy.isEmpty && !showLinks)
                      Padding(
                        padding: EdgeInsets.all(32),
                        child: Center(child: Text(tr('В этом разделе пусто'), style: TextStyle(color: AppTheme.textSecondary))),
                      ),
                    if (showLinks) ...[
                      Padding(
                        padding: const EdgeInsets.fromLTRB(16, 14, 16, 4),
                        child: Text(trf('Ссылки · {n}', {'n': widget.links.length}),
                            style: const TextStyle(color: AppTheme.accent, fontSize: 13, fontWeight: FontWeight.bold)),
                      ),
                      for (final l in widget.links)
                        ListTile(
                          leading: const Icon(Icons.link, color: AppTheme.textSecondary),
                          title: Text(l.url, maxLines: 2, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text)),
                          subtitle: Text(DateFormat('dd.MM.yyyy HH:mm').format(l.when),
                              style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                          // наружу сами не выходим: адрес копируется, открывать или нет — решает человек
                          onTap: () async {
                            await Clipboard.setData(ClipboardData(text: l.url));
                            if (!mounted) return;
                            ScaffoldMessenger.of(context).clearSnackBars();
                            ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Ссылка скопирована'))));
                          },
                        ),
                    ],
                    for (final kind in FileKind.values)
                      if (groups[kind] != null || missingBy[kind] != null) ...[
                        Padding(
                          padding: const EdgeInsets.fromLTRB(16, 14, 16, 4),
                          child: Text('${_kindTitles[kind]} · ${(groups[kind]?.length ?? 0) + (missingBy[kind]?.length ?? 0)}',
                              style: const TextStyle(color: AppTheme.accent, fontSize: 13, fontWeight: FontWeight.bold)),
                        ),
                        for (final o in missingBy[kind] ?? const <FileOffer>[])
                          ListTile(
                            key: ValueKey('missing-${o.tid}'),
                            leading: _fetching.contains(o.tid)
                                ? const SizedBox(width: 24, height: 24, child: CircularProgressIndicator(strokeWidth: 2))
                                : const Icon(Icons.cloud_download_outlined, color: AppTheme.textSecondary),
                            title: Text(o.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text)),
                            subtitle: Text(trf('не скачан · {size} · нажмите, чтобы скачать', {'size': _size(o.size)}),
                                style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                            onTap: _fetching.contains(o.tid) || widget.onFetch == null
                                ? null
                                : () async {
                                    setState(() => _fetching.add(o.tid));
                                    try { await widget.onFetch!(o); } finally {
                                      if (mounted) setState(() => _fetching.remove(o.tid));
                                    }
                                    await reload();
                                  },
                          ),
                        for (final f in groups[kind] ?? const <VaultFile>[])
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
