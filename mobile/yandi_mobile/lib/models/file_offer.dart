import 'dart:convert';

/// Предложение файла внутри чата. Едет обычным сообщением, которое телефон шифрует для получателя (E2E): в нём имя файла,
/// размер, номер передачи на узле, ключ шифрования кусков и контрольная сумма. Узел видит только зашифрованные куски.
class FileOffer {
  static const String marker = '\u0001yandi-file:';

  final String tid;      // номер передачи на узле
  final String name;
  final int    size;     // байт, в открытом виде
  final int    chunks;
  final String mime;
  final String key;      // base64, 32 байта
  final String sha;      // sha256 открытого файла, hex (пусто у исходящего сообщения, пока не посчитано)

  const FileOffer({
    required this.tid,
    required this.name,
    required this.size,
    required this.chunks,
    required this.mime,
    required this.key,
    this.sha = '',
  });

  String encode() => marker + jsonEncode({
    'v': 1, 'tid': tid, 'name': name, 'size': size, 'chunks': chunks, 'mime': mime, 'key': key, 'sha': sha,
  });

  FileOffer withSha(String s) => FileOffer(tid: tid, name: name, size: size, chunks: chunks, mime: mime, key: key, sha: s);

  static bool isOffer(String text) => text.startsWith(marker);

  static FileOffer? tryParse(String text) {
    if (!isOffer(text)) return null;
    try {
      final m = jsonDecode(text.substring(marker.length)) as Map<String, dynamic>;
      final tid = m['tid'] as String;
      final key = m['key'] as String;
      if (tid.length != 32 || base64.decode(key).length != 32) return null;
      return FileOffer(
        tid:    tid,
        name:   (m['name'] as String?) ?? 'file',
        size:   (m['size'] as num).toInt(),
        chunks: (m['chunks'] as num).toInt(),
        mime:   (m['mime'] as String?) ?? 'application/octet-stream',
        key:    key,
        sha:    (m['sha'] as String?) ?? '',
      );
    } catch (_) {
      return null;
    }
  }

  /// Короткая подпись для уведомлений и списков.
  static String preview(String text) {
    final o = tryParse(text);
    return o == null ? text : '📎 ${o.name}';
  }

  bool get isImage => mime.startsWith('image/');

  static String humanSize(int bytes) {
    if (bytes < 1024) return '$bytes Б';
    if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} КБ';
    if (bytes < 1024 * 1024 * 1024) return '${(bytes / 1024 / 1024).toStringAsFixed(1)} МБ';
    return '${(bytes / 1024 / 1024 / 1024).toStringAsFixed(2)} ГБ';
  }
}
