import 'dart:async';
import 'package:flutter/services.dart' show rootBundle;
import 'package:http/http.dart' as http;
import 'package:shared_preferences/shared_preferences.dart';
import '../models/bootstrap_list.dart';

/// Откуда взят список.
enum BootstrapSource { network, cache, embedded }

class BootstrapResult {
  final BootstrapDoc doc;
  final BootstrapSource source;
  const BootstrapResult(this.doc, this.source);
}

/// Подписанный список входных узлов для приложения: сначала сети (GitHub, затем сайт), потом копия на телефоне, потом вшитая копия.
/// Каждый вариант проходит ту же проверку (закреплённый ключ, подпись, срок, номер выпуска не меньше уже известного). Всё подставляется,
/// чтобы проверить без сети.
class BootstrapLoader {
  static const urls = <String>[
    'https://raw.githubusercontent.com/ispolkom/node/main/bootstrap/bootstrap.json',
    'https://yandi.su/update/bootstrap.json',
  ];
  static const _cacheKey = 'bootstrap_cache';

  final Future<String?> Function(String url) fetch;
  final Future<String?> Function() readCache;
  final Future<void> Function(String) writeCache;
  final Future<String> Function() embedded;
  final int Function() nowSecs;
  final List<String> signers;

  BootstrapLoader({
    Future<String?> Function(String url)? fetch,
    Future<String?> Function()? readCache,
    Future<void> Function(String)? writeCache,
    Future<String> Function()? embedded,
    int Function()? nowSecs,
    this.signers = bootstrapSigners,
  })  : fetch = fetch ?? _httpFetch,
        readCache = readCache ?? _prefsRead,
        writeCache = writeCache ?? _prefsWrite,
        embedded = embedded ?? (() => rootBundle.loadString('assets/bootstrap.json', cache: false)),
        nowSecs = nowSecs ?? (() => DateTime.now().millisecondsSinceEpoch ~/ 1000);

  static Future<String?> _httpFetch(String url) async {
    try {
      if (!url.startsWith('https://')) return null;
      final r = await http.get(Uri.parse(url)).timeout(const Duration(seconds: 8));
      if (r.statusCode != 200 || r.bodyBytes.length > 256 * 1024) return null;
      return r.body;
    } catch (_) {
      return null;
    }
  }

  static Future<String?> _prefsRead() async {
    try {
      return (await SharedPreferences.getInstance()).getString(_cacheKey);
    } catch (_) {
      return null;
    }
  }

  static Future<void> _prefsWrite(String s) async {
    try {
      await (await SharedPreferences.getInstance()).setString(_cacheKey, s);
    } catch (_) {}
  }

  Future<BootstrapDoc?> _good(String? text, {int? known}) async {
    if (text == null) return null;
    final d = BootstrapDoc.parse(text);
    if (d == null) return null;
    return await d.verify(nowSecs: nowSecs(), knownSequence: known, signers: signers) == null ? d : null;
  }

  /// Лучший проверенный список; null — ни один источник не дал верного (в том числе вшитая копия устарела).
  Future<BootstrapResult?> load() async {
    final cacheText = await readCache();
    final cached = await _good(cacheText);
    final emb = await _good(await embedded().catchError((_) => ''));
    // порог отката — самый большой из известных номеров выпуска
    final known = [cached?.sequence, emb?.sequence].whereType<int>().fold<int?>(null, (a, b) => a == null || b > a ? b : a);
    for (final u in urls) {
      final text = await fetch(u);
      final d = await _good(text, known: known);
      if (d != null) {
        await writeCache(text!);
        return BootstrapResult(d, BootstrapSource.network);
      }
    }
    if (cached != null && (emb == null || cached.sequence >= emb.sequence)) return BootstrapResult(cached, BootstrapSource.cache);
    if (emb != null) return BootstrapResult(emb, BootstrapSource.embedded);
    return cached == null ? null : BootstrapResult(cached, BootstrapSource.cache);
  }
}
