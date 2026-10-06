import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../services/file_transfer_service.dart';
import '../theme.dart';

class FilesScreen extends StatefulWidget {
  const FilesScreen({super.key});
  @override
  State<FilesScreen> createState() => _FilesScreenState();
}

class _FilesScreenState extends State<FilesScreen> {
  List<Map<String, dynamic>> _files  = [];
  bool                       _loading = true;

  // transferId → прогресс загрузки
  final Map<String, double> _progress = {};

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    setState(() => _loading = true);
    final state = context.read<AppState>();
    if (state.activeNode == null) {
      setState(() => _loading = false);
      return;
    }
    final files = await state.apiService.listFiles();
    if (!mounted) return;
    setState(() { _files = files; _loading = false; });
  }

  Future<void> _download(Map<String, dynamic> f) async {
    final id    = f['id']           as String;
    final name  = f['file_name']    as String;
    final total = (f['total_chunks'] as num).toInt();

    setState(() => _progress[id] = 0.0);

    final state = context.read<AppState>();
    final svc   = FileTransferService(state.apiService);

    final path = await svc.downloadFile(
      transferId:  id,
      fileName:    name,
      totalChunks: total,
      onProgress: (p) {
        if (!mounted) return;
        setState(() => _progress[id] = p.fraction);
      },
    );

    if (!mounted) return;
    setState(() => _progress.remove(id));

    ScaffoldMessenger.of(context).showSnackBar(SnackBar(
      content: Text(path != null ? 'Сохранено: $name' : 'Ошибка загрузки'),
      backgroundColor: path != null ? AppTheme.accent : Colors.redAccent,
    ));
  }

  Future<void> _delete(String id) async {
    await context.read<AppState>().apiService.deleteTransfer(id);
    setState(() => _files.removeWhere((f) => f['id'] == id));
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        elevation: 0,
        title: const Text('Входящие файлы',
            style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh, color: AppTheme.textSecondary),
            onPressed: _load,
          ),
        ],
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator(color: AppTheme.accent))
          : _files.isEmpty
              ? const Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.folder_open, color: AppTheme.textSecondary, size: 64),
                      SizedBox(height: 16),
                      Text('Нет входящих файлов',
                          style: TextStyle(color: AppTheme.textSecondary)),
                    ],
                  ),
                )
              : ListView.builder(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  itemCount: _files.length,
                  itemBuilder: (_, i) => _FileTile(
                    file:       _files[i],
                    progress:   _progress[_files[i]['id'] as String],
                    onDownload: () => _download(_files[i]),
                    onDelete:   () => _delete(_files[i]['id'] as String),
                  ),
                ),
    );
  }
}

class _FileTile extends StatelessWidget {
  final Map<String, dynamic> file;
  final double?    progress;
  final VoidCallback onDownload;
  final VoidCallback onDelete;

  const _FileTile({
    required this.file,
    required this.onDownload,
    required this.onDelete,
    this.progress,
  });

  @override
  Widget build(BuildContext context) {
    final name      = file['file_name']   as String;
    final size      = (file['file_size']  as num).toInt();
    final complete  = (file['complete']   as bool?) ?? false;
    final fromId    = (file['from_peer_id'] as String?) ?? '';
    final shortFrom = fromId.length > 12 ? fromId.substring(0, 12) : fromId;

    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: AppTheme.surface,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                _fileIcon(name),
                color: AppTheme.accent,
                size: 28,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(name,
                        style: const TextStyle(
                            color: AppTheme.text,
                            fontWeight: FontWeight.w600,
                            fontSize: 14),
                        overflow: TextOverflow.ellipsis),
                    const SizedBox(height: 2),
                    Text(
                      '${_formatBytes(size)}  •  от $shortFrom',
                      style: const TextStyle(
                          color: AppTheme.textSecondary, fontSize: 11),
                    ),
                  ],
                ),
              ),
              // Кнопка скачать / удалить
              if (progress == null) ...[
                if (complete)
                  IconButton(
                    icon: const Icon(Icons.download, color: AppTheme.accent),
                    onPressed: onDownload,
                    tooltip: 'Скачать',
                  ),
                IconButton(
                  icon: const Icon(Icons.delete_outline,
                      color: AppTheme.textSecondary),
                  onPressed: onDelete,
                  tooltip: 'Удалить',
                ),
              ] else
                const SizedBox(
                    width: 24, height: 24,
                    child: CircularProgressIndicator(
                        strokeWidth: 2, color: AppTheme.accent)),
            ],
          ),
          // Прогресс-бар при скачивании
          if (progress != null) ...[
            const SizedBox(height: 8),
            ClipRRect(
              borderRadius: BorderRadius.circular(4),
              child: LinearProgressIndicator(
                value: progress,
                backgroundColor: AppTheme.bg,
                color: AppTheme.accent,
                minHeight: 4,
              ),
            ),
          ],
          // Индикатор незавершённой передачи
          if (!complete && progress == null)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                'Загружается ${file['chunks_done']}/${file['total_chunks']} частей',
                style: const TextStyle(
                    color: AppTheme.textSecondary, fontSize: 11),
              ),
            ),
        ],
      ),
    );
  }

  IconData _fileIcon(String name) {
    final ext = name.contains('.') ? name.split('.').last.toLowerCase() : '';
    return switch (ext) {
      'jpg' || 'jpeg' || 'png' || 'gif' || 'webp' => Icons.image_outlined,
      'mp4' || 'mov' || 'avi' || 'mkv'            => Icons.videocam_outlined,
      'mp3' || 'ogg' || 'aac' || 'flac'           => Icons.audiotrack_outlined,
      'pdf'                                        => Icons.picture_as_pdf_outlined,
      'zip' || 'rar' || 'tar' || 'gz'             => Icons.archive_outlined,
      _                                            => Icons.insert_drive_file_outlined,
    };
  }

  static String _formatBytes(int bytes) {
    if (bytes < 1024)            return '$bytes Б';
    if (bytes < 1024 * 1024)     return '${(bytes / 1024).toStringAsFixed(1)} КБ';
    if (bytes < 1024 * 1024 * 1024) return '${(bytes / 1024 / 1024).toStringAsFixed(1)} МБ';
    return '${(bytes / 1024 / 1024 / 1024).toStringAsFixed(1)} ГБ';
  }
}
