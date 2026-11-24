/// Политика содержимого YANDI: ссылки не передаются, вложения — только из белого списка.
/// Те же правила, что у узла (src/content_policy.rs) и у оболочки ПК: менять только вместе.
library;

import 'dart:convert';

import 'dart:io';

const String linkRemoved = '[ссылка удалена]';

const Set<String> allowedExt = {
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'heif', 'avif', 'bmp',
  'mp3', 'm4a', 'aac', 'ogg', 'oga', 'opus', 'wav', 'flac', 'amr', 'mid', 'midi',
  'mp4', 'mov', 'mkv', 'webm', '3gp',
  'pdf', 'txt', 'csv', 'docx', 'xlsx', 'pptx', 'odt', 'ods', 'odp', 'xml',
};

const Set<String> bannedExt = {
  'exe', 'bat', 'cmd', 'com', 'msi', 'scr', 'pif', 'dll', 'sys', 'lnk', 'reg', 'sh', 'bash', 'ps1', 'psm1', 'vbs', 'vbe', 'wsf', 'wsh',
  'js', 'jse', 'mjs', 'jar', 'apk', 'aab', 'ipa', 'deb', 'rpm', 'appimage', 'py', 'pyc', 'rb', 'pl', 'php', 'html', 'htm', 'xhtml',
  'svg', 'hta', 'cpl', 'chm', 'iso', 'img', 'dmg', 'zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'bz2', 'xz', 'cab', 'doc', 'xls', 'ppt',
  'docm', 'xlsm', 'pptm', 'dotm', 'xlam', 'rtf', 'swf', 'url',
};

const Set<String> _tlds = {
  'com', 'net', 'org', 'info', 'biz', 'edu', 'gov', 'mil', 'int', 'name', 'pro', 'mobi', 'tel', 'asia', 'travel', 'jobs', 'museum',
  'xxx', 'app', 'dev', 'page', 'blog', 'shop', 'store', 'online', 'site', 'website', 'space', 'tech', 'cloud', 'live', 'life', 'world',
  'today', 'news', 'media', 'video', 'tube', 'stream', 'download', 'link', 'click', 'xyz', 'top', 'club', 'vip', 'fun', 'icu', 'cyou',
  'buzz', 'win', 'bid', 'loan', 'work', 'party', 'date', 'trade', 'cam', 'bar', 'rest', 'pub', 'wiki', 'cc', 'tv', 'ws', 'gg', 'ly',
  'to', 'su', 'рф', 'москва', 'онлайн', 'сайт', 'орг', 'ком', 'бел', 'укр', 'дети', 'рус', 'срб', 'католик', 'ею', 'tk', 'ml', 'ga',
  'cf', 'gq', 'zip', 'mov', 'bank', 'casino', 'poker', 'bet', 'porn', 'sex', 'adult', 'ninja', 'rocks', 'guru', 'email', 'chat',
  'social', 'network', 'digital', 'systems', 'solutions', 'services', 'agency', 'company', 'center', 'group', 'team', 'zone', 'host',
  'domains', 'hosting', 'software', 'tools', 'codes', 'run', 'ink', 'art', 'design', 'photo', 'pics', 'pictures', 'gallery', 'studio',
};

bool _isTld(String zone) {
  final z = zone.toLowerCase();
  if (z.startsWith('xn--')) return true;
  if (allowedExt.contains(z) && z.length > 2) return false;
  if (_tlds.contains(z) || bannedExt.contains(z)) return true;
  return z.length == 2 && RegExp(r'^[a-z]{2}$').hasMatch(z) && !allowedExt.contains(z);
}

final RegExp _scheme = RegExp(r'[a-z][a-z0-9+.\-]{1,15}://\S*|www\.\S+|\bmailto:\S+', caseSensitive: false);
final RegExp _domain = RegExp(
  r'(?:[\p{L}\p{N}](?:[\p{L}\p{N}\-]{0,61}[\p{L}\p{N}])?[.。．｡])+(xn--[a-z0-9\-]{2,40}|[\p{L}\p{N}]{2,24})(?:[/:?#]\S*)?',
  caseSensitive: false,
  unicode: true,
);

/// Диапазоны ссылок [start, end) в тексте, по порядку, без пересечений.
List<(int, int)> linkRanges(String text) {
  final v = <(int, int)>[
    for (final m in _scheme.allMatches(text)) (m.start, m.end),
  ];
  for (final m in _domain.allMatches(text)) {
    final zone = m.group(1)!;
    if (m.start > 0) {
      final prev = text.substring(m.start - 1, m.start);
      if (RegExp(r'[\p{L}\p{N}@]', unicode: true).hasMatch(prev)) continue;
    }
    if (!_isTld(zone)) continue;
    v.add((m.start, m.end));
  }
  v.sort((a, b) => a.$1.compareTo(b.$1));
  final out = <(int, int)>[];
  for (final r in v) {
    if (out.isNotEmpty && r.$1 <= out.last.$2) {
      final last = out.removeLast();
      out.add((last.$1, last.$2 > r.$2 ? last.$2 : r.$2));
    } else {
      out.add(r);
    }
  }
  return out;
}

/// Первая найденная ссылка (чтобы показать отправителю), либо null.
String? findLink(String text) {
  final r = linkRanges(text);
  return r.isEmpty ? null : text.substring(r.first.$1, r.first.$2);
}

/// Текст без ссылок: каждая заменена на [linkRemoved].
String stripLinks(String text) {
  final ranges = linkRanges(text);
  if (ranges.isEmpty) return text;
  final b = StringBuffer();
  var at = 0;
  for (final (s, e) in ranges) {
    b.write(text.substring(at, s));
    b.write(linkRemoved);
    at = e;
  }
  b.write(text.substring(at));
  return b.toString();
}

/// Текст без ссылок с удалённой ссылкой для отправителя («Удалить ссылку»).
String removeLinks(String text) => stripLinks(text).replaceAll(linkRemoved, '').replaceAll(RegExp(r'[ \t]{2,}'), ' ').trim();

/// Имя человека или контакта: без ссылок и управляющих знаков, один пробел между словами, не длиннее 64 знаков.
String cleanPersonName(String raw) {
  final t = removeLinks(raw).replaceAll(RegExp(r'[\x00-\x1f\x7f]'), ' ').replaceAll(RegExp(r'\s{2,}'), ' ').trim();
  final runes = t.runes.toList();
  return runes.length > 64 ? String.fromCharCodes(runes.take(64)).trim() : t;
}

String? _extOf(String name) {
  final base = name.split(RegExp(r'[/\\]')).last.trim();
  final i = base.lastIndexOf('.');
  if (i < 0 || i == base.length - 1) return null;
  return base.substring(i + 1).toLowerCase();
}

/// null — имя вложения допустимо; иначе объяснение отказа.
String? attachmentProblem(String name) {
  final base = name.split(RegExp(r'[/\\]')).last.trim();
  final ext = _extOf(base);
  if (ext == null) return 'Файл без расширения не передаётся';
  if (!allowedExt.contains(ext)) {
    return 'Файлы «.$ext» в YANDI не передаются (разрешены картинки, аудио, видео и документы без макросов)';
  }
  final parts = base.split('.').skip(1).toList();
  for (final p in parts.take(parts.length - 1)) {
    if (bannedExt.contains(p.toLowerCase())) return 'В имени файла «.$p»: так замаскированные файлы не передаются';
  }
  return null;
}

bool _textLike(List<int> head) {
  if (head.isEmpty || head.contains(0)) return false;
  // корректный UTF-8; обрыв многобайтового знака на краю головы допустим (как в Rust)
  for (var cut = 0; cut <= 3 && cut < head.length; cut++) {
    try {
      utf8.decode(head.sublist(0, head.length - cut));
      return cut == 0 || _tailIsPartialChar(head, cut);
    } on FormatException {
      continue;
    }
  }
  return false;
}

/// Хвост из `cut` байт — начало многобайтового знака, оборванного границей головы (а не испорченные данные).
bool _tailIsPartialChar(List<int> head, int cut) {
  final start = head.length - cut;
  final lead = head[start];
  final need = lead >= 0xF0 ? 4 : lead >= 0xE0 ? 3 : lead >= 0xC0 ? 2 : 0;
  if (need == 0 || cut >= need) return false;
  for (var i = start + 1; i < head.length; i++) {
    if (head[i] & 0xC0 != 0x80) return false;
  }
  return true;
}

/// Первая запись ZIP-контейнера должна быть из набора документа: произвольный архив под видом .docx не проходит.
bool _officeContainerOk(String ext, List<int> head) {
  if (head.length < 4 || head[0] != 0x50 || head[1] != 0x4b || head[2] != 0x03 || head[3] != 0x04) return false;
  if (head.length < 30) return true;
  final n = head[26] | (head[27] << 8);
  if (head.length < 30 + n) return true;
  final name = String.fromCharCodes(head.sublist(30, 30 + n)).toLowerCase();
  if (ext == 'odt' || ext == 'ods' || ext == 'odp') return name == 'mimetype';
  return name == '[content_types].xml' ||
      ['_rels/', 'docprops/', 'word/', 'xl/', 'ppt/', 'customxml/'].any(name.startsWith);
}

/// Проверка имени и первых байтов: исполняемое под любым именем отклоняется, содержимое должно соответствовать расширению.
String? contentProblem(String name, List<int> head) {
  final p = attachmentProblem(name);
  if (p != null) return p;
  final ext = _extOf(name)!;
  bool starts(List<int> s) => head.length >= s.length && [for (var i = 0; i < s.length; i++) head[i] == s[i]].every((e) => e);
  List<int> a(String s) => s.codeUnits;
  bool has(String s, int from) => head.length >= from + s.length && [for (var i = 0; i < s.length; i++) head[from + i] == s.codeUnitAt(i)].every((e) => e);
  if (ext == 'amr' && starts(a('#!AMR'))) return null;
  if (starts(a('MZ')) || starts([0x7f, 0x45, 0x4c, 0x46]) || starts(a('#!')) || starts([0xca, 0xfe, 0xba, 0xbe])) {
    return 'Внутри исполняемый файл под видом документа: не передаётся';
  }
  final ok = switch (ext) {
    'png' => starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    'jpg' || 'jpeg' => starts([0xff, 0xd8, 0xff]),
    'gif' => starts(a('GIF87a')) || starts(a('GIF89a')),
    'webp' => starts(a('RIFF')) && has('WEBP', 8),
    'bmp' => starts(a('BM')),
    'heic' || 'heif' || 'avif' || 'mp4' || 'mov' || 'm4a' || '3gp' => has('ftyp', 4),
    'pdf' => starts(a('%PDF-')),
    'mp3' => starts(a('ID3')) || (head.length > 2 && head[0] == 0xff && (head[1] & 0xe0) == 0xe0),
    'aac' => head.length > 2 && head[0] == 0xff && (head[1] & 0xf0) == 0xf0,
    'ogg' || 'oga' || 'opus' => starts(a('OggS')),
    'wav' => starts(a('RIFF')) && has('WAVE', 8),
    'flac' => starts(a('fLaC')),
    'amr' => starts(a('#!AMR')),
    'mid' || 'midi' => starts(a('MThd')),
    'mkv' || 'webm' => starts([0x1a, 0x45, 0xdf, 0xa3]),
    'docx' || 'xlsx' || 'pptx' || 'odt' || 'ods' || 'odp' => _officeContainerOk(ext, head),
    'txt' || 'csv' || 'xml' => _textLike(head),
    _ => false,
  };
  return ok ? null : 'Содержимое не похоже на «.$ext»: файл не передаётся';
}

/// Проверка файла перед отправкой: имя по белому списку и первые байты содержимого. null — можно отправлять.
/// Чтение синхронное и короткое (64 байта): так проверка не зависит от очереди событий и не мешает выбору файла.
String? fileVerdictSync(String path) {
  final name = path.split(RegExp(r'[/\\]')).last;
  final byName = attachmentProblem(name);
  if (byName != null) return byName;
  try {
    final raf = File(path).openSync();
    try {
      return contentProblem(name, raf.readSync(64));
    } finally {
      raf.closeSync();
    }
  } catch (_) {
    return 'Не удалось прочитать файл для проверки';
  }
}

Future<String?> fileVerdict(String path) async => fileVerdictSync(path);
