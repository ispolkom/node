import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' show sha256;
import 'package:cryptography/cryptography.dart';

/// Зашифрованный файл хранилища (`<номер>.yfe`): имя, тип и размер лежат внутри (на диске видно только безликий номер), содержимое
/// шифруется кусками по 1 МБ (AES-256-GCM) отдельным ключом файла, выведенным из ключа данных и номера файла.
/// Каждый кусок проверяется вместе с заголовком, номером и признаком «последний»: переставить, подменить, обрезать или дописать
/// кусок незаметно нельзя; читать можно с любого места (нужные куски расшифровываются отдельно).
///
/// Формат: `YFE1` | версия 1 | 3 нуля | номер файла 16 | префикс nonce 4 | размер куска u32 | длина мета u16 | мета (nonce 12 + шифртекст JSON +
/// метка 16) | куски (шифртекст + метка 16). Nonce куска = префикс + номер куска; AAD куска = первые 34 байта заголовка + номер + «последний».
class FileVaultException implements Exception {
  final String message;
  FileVaultException(this.message);
  @override
  String toString() => message;
}

class FileMeta {
  final String name, mime;
  final int size, createdMs;
  const FileMeta({required this.name, required this.mime, required this.size, required this.createdMs});
}

class FileVault {
  FileVault.withKey(List<int> rootKey, {this.chunkSize = defaultChunkSize}) : _root = rootKey;

  final List<int> _root;
  final int chunkSize;

  static const defaultChunkSize = 1 << 20;
  static const extension = '.yfe';
  static const _magic = [0x59, 0x46, 0x45, 0x31];
  static const _fixedLen = 34;
  static const _tag = 16;
  static final _aes = AesGcm.with256bits();
  static final _hkdf = Hkdf(hmac: Hmac.sha256(), outputLength: 32);

  /// Блокировка: ключ из памяти стирается (повторное использование этого объекта даст ошибку проверки, а не данные).
  void dispose() {
    final r = _root;
    if (r is Uint8List) r.fillRange(0, r.length, 0);
  }

  static bool isVaultPath(String path) => path.endsWith(extension);

  /// Безопасное имя файла по номеру передачи: номер от узла — 32 hex-знака; всё прочее (чужое предложение с путями и т. п.) сводится
  /// к такой же строке по хэшу, так что на диске никогда не окажется ничего, кроме `[0-9a-f]{32}`.
  static String safeId(String raw) {
    if (RegExp(r'^[0-9a-f]{32}$').hasMatch(raw)) return raw;
    return sha256.convert(utf8.encode(raw)).toString().substring(0, 32);
  }

  /// Номер файла по пути (`…/<номер>.yfe`) — он же имя на диске.
  static String idOf(String path) {
    final n = path.split(Platform.pathSeparator).last;
    return n.endsWith(extension) ? n.substring(0, n.length - extension.length) : n;
  }

  // ── Запись ───────────────────────────────────────────────────────────────────────────────────────────────────────

  /// Начать запись файла известного размера. Данные подаются через [VaultFileWriter.add], в конце — [VaultFileWriter.close].
  Future<VaultFileWriter> create(File target, {required String name, required String mime, required int size, int? createdMs}) async {
    if (size < 0) throw FileVaultException('размер файла');
    final rnd = Random.secure();
    Uint8List rb(int n) => Uint8List.fromList(List.generate(n, (_) => rnd.nextInt(256)));
    final fileId = rb(16), noncePrefix = rb(4);
    final meta = utf8.encode(jsonEncode({'n': name, 'm': mime, 's': size, 't': createdMs ?? DateTime.now().millisecondsSinceEpoch}));
    final metaLen = 12 + meta.length + _tag;
    if (metaLen > 0xFFFF) throw FileVaultException('имя файла слишком длинное');
    final fixed = _fixedHeader(fileId, noncePrefix, chunkSize, metaLen);
    final key = await _fileKey(fileId);
    final metaNonce = rb(12);
    final box = await _aes.encrypt(meta, secretKey: key, nonce: metaNonce, aad: [...fixed, ..._metaAad]);
    final sink = target.openWrite();
    sink.add(fixed);
    sink.add([...metaNonce, ...box.cipherText, ...box.mac.bytes]);
    return VaultFileWriter._(this, target, sink, key, fixed, noncePrefix, size);
  }

  /// Положить копию обычного файла в хранилище (исходный не трогаем).
  Future<FileMeta> importFile(File src, File target, {required String name, required String mime}) async {
    final size = await src.length();
    final w = await create(target, name: name, mime: mime, size: size);
    try {
      await for (final part in src.openRead()) {
        await w.add(part);
      }
      await w.close();
    } catch (e) {
      await w.abort();
      rethrow;
    }
    return FileMeta(name: name, mime: mime, size: size, createdMs: DateTime.now().millisecondsSinceEpoch);
  }

  // ── Чтение ───────────────────────────────────────────────────────────────────────────────────────────────────────

  /// Только сведения о файле (имя, тип, размер): читается начало файла, содержимое не расшифровывается.
  Future<FileMeta> readMeta(File f) async {
    final raf = await f.open();
    try {
      return (await _parse(raf)).meta;
    } finally {
      await raf.close();
    }
  }

  /// Весь файл по порядку, куски по одному.
  Stream<Uint8List> read(File f) async* {
    final raf = await f.open();
    try {
      final p = await _parse(raf);
      for (var i = 0; i < p.chunks; i++) {
        yield await _chunk(raf, p, i);
      }
    } finally {
      await raf.close();
    }
  }

  /// Часть файла: расшифровываются только нужные куски.
  Future<Uint8List> readRange(File f, int pos, int len) async {
    final raf = await f.open();
    try {
      final p = await _parse(raf);
      final end = min(pos + len, p.meta.size);
      if (pos < 0 || pos >= end) return Uint8List(0);
      final out = BytesBuilder(copy: false);
      for (var i = pos ~/ p.chunk; i * p.chunk < end; i++) {
        final plain = await _chunk(raf, p, i);
        final from = max(pos, i * p.chunk) - i * p.chunk;
        final to = min(end, (i + 1) * p.chunk) - i * p.chunk;
        out.add(Uint8List.sublistView(plain, from, to));
      }
      return out.toBytes();
    } finally {
      await raf.close();
    }
  }

  /// Размер куска файла (из заголовка; ключ не нужен).
  Future<int> chunkSizeOf(File f) async {
    final raf = await f.open();
    try {
      final fixed = await raf.read(_fixedLen);
      if (fixed.length < _fixedLen || !_listEq(fixed.sublist(0, 4), _magic)) throw FileVaultException('это не файл хранилища');
      return ByteData.sublistView(fixed).getUint32(28);
    } finally {
      await raf.close();
    }
  }

  /// Первые [limit] байт (или весь файл, если он короче) — для текстового просмотра.
  Future<Uint8List> readPrefix(File f, int limit) => readRange(f, 0, limit);

  Future<Uint8List> readAll(File f) async {
    final m = await readMeta(f);
    return readRange(f, 0, m.size);
  }

  /// Расшифровать в обычный файл (временная копия для отправки или сохранения в «Загрузки»). Файл потом надо затереть.
  Future<void> exportTo(File src, File dst) async {
    final sink = dst.openWrite();
    try {
      await for (final c in read(src)) {
        sink.add(c);
      }
      await sink.flush();
      await sink.close();
    } catch (e) {
      try { await sink.close(); } catch (_) {}
      try { await dst.delete(); } catch (_) {}
      rethrow;
    }
  }

  // ── Внутреннее ───────────────────────────────────────────────────────────────────────────────────────────────────

  static const _metaAad = [0x6d, 0x65, 0x74, 0x61]; // "meta"

  Future<SecretKey> _fileKey(List<int> fileId) =>
      _hkdf.deriveKey(secretKey: SecretKey(_root), nonce: fileId, info: utf8.encode('yandi-file-key:v1'));

  static Uint8List _fixedHeader(Uint8List fileId, Uint8List noncePrefix, int chunk, int metaLen) {
    final b = BytesBuilder();
    b.add(_magic);
    b.addByte(1);
    b.add([0, 0, 0]);
    b.add(fileId);
    b.add(noncePrefix);
    b.add((ByteData(4)..setUint32(0, chunk)).buffer.asUint8List());
    b.add((ByteData(2)..setUint16(0, metaLen)).buffer.asUint8List());
    return b.toBytes();
  }

  static List<int> _nonce(List<int> prefix, int index) => [...prefix, ...(ByteData(8)..setUint64(0, index)).buffer.asUint8List()];
  static List<int> _chunkAad(List<int> fixed, int index, bool last) =>
      [...fixed, ...(ByteData(8)..setUint64(0, index)).buffer.asUint8List(), last ? 1 : 0];

  Future<_Parsed> _parse(RandomAccessFile raf) async {
    final total = await raf.length();
    await raf.setPosition(0);
    final fixed = await raf.read(_fixedLen);
    if (fixed.length < _fixedLen || !_listEq(fixed.sublist(0, 4), _magic) || fixed[4] != 1) throw FileVaultException('это не файл хранилища');
    final bd = ByteData.sublistView(fixed);
    final chunk = bd.getUint32(28);
    final metaLen = bd.getUint16(32);
    if (chunk < 1024 || chunk > (64 << 20) || metaLen < 12 + _tag) throw FileVaultException('заголовок файла повреждён');
    final blob = await raf.read(metaLen);
    if (blob.length != metaLen) throw FileVaultException('файл обрезан');
    final key = await _fileKey(fixed.sublist(8, 24));
    FileMeta meta;
    try {
      final box = SecretBox(blob.sublist(12, metaLen - _tag), nonce: blob.sublist(0, 12), mac: Mac(blob.sublist(metaLen - _tag)));
      final j = jsonDecode(utf8.decode(await _aes.decrypt(box, secretKey: key, aad: [...fixed, ..._metaAad]))) as Map<String, dynamic>;
      meta = FileMeta(name: j['n'] as String, mime: j['m'] as String, size: j['s'] as int, createdMs: j['t'] as int);
    } catch (_) {
      throw FileVaultException('файл не открывается: другой ключ или повреждён');
    }
    final chunks = meta.size == 0 ? 0 : (meta.size + chunk - 1) ~/ chunk;
    final expected = _fixedLen + metaLen + meta.size + chunks * _tag;
    if (total != expected) throw FileVaultException('файл обрезан или дописан');
    return _Parsed(fixed: Uint8List.fromList(fixed), key: key, chunk: chunk, chunks: chunks, meta: meta, dataStart: _fixedLen + metaLen);
  }

  Future<Uint8List> _chunk(RandomAccessFile raf, _Parsed p, int i) async {
    final plainLen = i == p.chunks - 1 ? p.meta.size - i * p.chunk : p.chunk;
    await raf.setPosition(p.dataStart + i * (p.chunk + _tag));
    final blob = await raf.read(plainLen + _tag);
    if (blob.length != plainLen + _tag) throw FileVaultException('файл обрезан');
    try {
      final box = SecretBox(blob.sublist(0, plainLen), nonce: _nonce(p.fixed.sublist(24, 28), i), mac: Mac(blob.sublist(plainLen)));
      return Uint8List.fromList(await _aes.decrypt(box, secretKey: p.key, aad: _chunkAad(p.fixed, i, i == p.chunks - 1)));
    } catch (_) {
      throw FileVaultException('кусок файла не прошёл проверку: файл повреждён или подменён');
    }
  }

  static bool _listEq(List<int> a, List<int> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }
}

class _Parsed {
  final Uint8List fixed;
  final SecretKey key;
  final int chunk, chunks, dataStart;
  final FileMeta meta;
  _Parsed({required this.fixed, required this.key, required this.chunk, required this.chunks, required this.meta, required this.dataStart});
}

/// Запись зашифрованного файла известного размера. Недописанный или неполный файл не остаётся: [abort] и неверный размер его удаляют.
class VaultFileWriter {
  VaultFileWriter._(FileVault v, this._file, this._sink, this._key, this._fixed, this._noncePrefix, this.size)
      : _chunk = v.chunkSize,
        _total = size == 0 ? 0 : (size + v.chunkSize - 1) ~/ v.chunkSize,
        _buf = Uint8List(v.chunkSize);

  final File _file;
  final IOSink _sink;
  final SecretKey _key;
  final Uint8List _fixed, _noncePrefix;
  final int size, _chunk, _total;
  final Uint8List _buf;
  int _fill = 0, _index = 0, _written = 0;
  bool _done = false;

  Future<void> add(List<int> data) async {
    if (_done) throw FileVaultException('запись уже закончена');
    var off = 0;
    while (off < data.length) {
      final n = min(_chunk - _fill, data.length - off);
      if (_written + n > size) {
        await abort();
        throw FileVaultException('данных больше заявленного размера');
      }
      _buf.setRange(_fill, _fill + n, data, off);
      _fill += n;
      off += n;
      _written += n;
      // последний кусок оставляем до close(): ему нужен признак «последний»
      if (_fill == _chunk && _index < _total - 1) await _flush(false);
    }
  }

  Future<void> close() async {
    if (_done) return;
    if (_written != size) {
      await abort();
      throw FileVaultException('данных меньше заявленного размера');
    }
    try {
      if (_total > 0) await _flush(true);
      await _sink.flush();
      await _sink.close();
      _done = true;
    } catch (e) {
      await abort();
      rethrow;
    }
  }

  Future<void> abort() async {
    if (_done) return;
    _done = true;
    try { await _sink.close(); } catch (_) {}
    try { await _file.delete(); } catch (_) {}
  }

  Future<void> _flush(bool last) async {
    final plain = Uint8List.sublistView(_buf, 0, _fill);
    final box = await FileVault._aes.encrypt(plain, secretKey: _key, nonce: FileVault._nonce(_noncePrefix, _index), aad: FileVault._chunkAad(_fixed, _index, last));
    _sink.add(box.cipherText);
    _sink.add(box.mac.bytes);
    _fill = 0;
    _index++;
  }
}

/// Чтение файла для отправки: обычный файл или файл хранилища читаются одинаково — «дай len байт с позиции pos».
abstract class FileSource {
  int get length;
  Future<Uint8List> readAt(int pos, int len);
  Future<void> close();

  /// [vault] нужен для файлов `.yfe`; обычные открываются как есть.
  static Future<FileSource> open(String path, {FileVault? vault}) async {
    if (FileVault.isVaultPath(path)) {
      if (vault == null) throw FileVaultException('хранилище заблокировано');
      final f = File(path);
      return _VaultSource(vault, f, (await vault.readMeta(f)).size, await vault.chunkSizeOf(f));
    }
    final raf = await File(path).open();
    return _PlainSource(raf, await raf.length());
  }
}

/// Параллельные чтения (отправка в группу идёт в несколько потоков) выстраиваются в очередь: у одного открытого файла позиция одна.
mixin _Serial {
  Future<void> _tail = Future.value();
  Future<T> serial<T>(Future<T> Function() op) {
    final r = _tail.then((_) => op());
    _tail = r.then((_) {}, onError: (_) {});
    return r;
  }
}

class _PlainSource with _Serial implements FileSource {
  _PlainSource(this._raf, this.length);
  final RandomAccessFile _raf;
  @override
  final int length;
  @override
  Future<Uint8List> readAt(int pos, int len) => serial(() => _readAt(pos, len));

  Future<Uint8List> _readAt(int pos, int len) async {
    await _raf.setPosition(pos);
    final want = min(len, length - pos);
    final buf = Uint8List(want);
    var got = 0;
    while (got < want) {
      final n = await _raf.readInto(buf, got, want);
      if (n <= 0) throw FileVaultException('файл изменился во время чтения');
      got += n;
    }
    return buf;
  }

  @override
  Future<void> close() => _raf.close();
}

class _VaultSource with _Serial implements FileSource {
  _VaultSource(this._v, this._f, this.length, this._chunk);
  final FileVault _v;
  final File _f;
  final int _chunk;
  @override
  final int length;
  int _cachedIndex = -1;
  Uint8List _cached = Uint8List(0);

  /// Читаем кусками файла и помним последний: подряд идущие чтения по 256 КБ не расшифровывают один и тот же кусок заново.
  @override
  Future<Uint8List> readAt(int pos, int len) => serial(() => _readAt(pos, len));

  Future<Uint8List> _readAt(int pos, int len) async {
    final end = min(pos + len, length);
    final out = BytesBuilder(copy: false);
    var p = pos;
    while (p < end) {
      final idx = p ~/ _chunk;
      if (idx != _cachedIndex) {
        _cached = await _v.readRange(_f, idx * _chunk, _chunk);
        _cachedIndex = idx;
      }
      final from = p - idx * _chunk;
      final to = min(end - idx * _chunk, _cached.length);
      out.add(Uint8List.sublistView(_cached, from, to));
      p = idx * _chunk + to;
    }
    return out.toBytes();
  }

  @override
  Future<void> close() async {
    _cached = Uint8List(0);
  }
}
