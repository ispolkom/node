import 'dart:convert';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;

/// Служебные E2E-конверты для аватарок между устройствами (как квитанции — внутри обычного зашифрованного сообщения):
///   avh — «моя картинка сейчас такая» (хэш; пустой — картинки нет/убрана),
///   avq — «пришли картинку с этим хэшем»,
///   ava — сама картинка (PNG, base64) и её хэш.
/// Хэш — SHA-256 самих байт картинки, поэтому у одинаковых картинок один файл, а подмену ловит проверка хэша. Подлинность отправителя
/// даёт подпись E4 (картинка чужого устройства не может прийти «от» другого). Узел видит только шифртекст.
class AvatarMsg {
  static const hMarker = '\u0001yandi-avh:';
  static const qMarker = '\u0001yandi-avq:';
  static const aMarker = '\u0001yandi-ava:';

  /// Картинка до 40 КБ, чтобы вместе с base64 и шифрованием уложиться в предел одного сообщения узла (64 КБ).
  static const maxPng = 40 * 1024;
  static final _hex64 = RegExp(r'^[0-9a-f]{64}$');
  static const _pngMagic = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

  final String kind;      // 'h' | 'q' | 'a'
  final String hash;      // 64 hex; у 'h' может быть пустым
  final Uint8List? png;   // только у 'a'
  const AvatarMsg._(this.kind, this.hash, this.png);

  static String announce(String hash) => hMarker + jsonEncode({'h': hash});
  static String request(String hash) => qMarker + jsonEncode({'h': hash});
  static String image(String hash, Uint8List png) => aMarker + jsonEncode({'h': hash, 'png': base64.encode(png)});

  static bool isAvatar(String t) => t.startsWith(hMarker) || t.startsWith(qMarker) || t.startsWith(aMarker);

  static String hashOf(Uint8List png) => crypto.sha256.convert(png).toString();

  static bool validHash(String h) => _hex64.hasMatch(h);

  /// PNG по сигнатуре и размеру (содержимое картинки декодером проверяет получатель отдельно).
  static bool validPng(Uint8List b) {
    if (b.length < 16 || b.length > maxPng) return false;
    for (var i = 0; i < _pngMagic.length; i++) { if (b[i] != _pngMagic[i]) return false; }
    return true;
  }

  static AvatarMsg? tryParse(String t) {
    try {
      if (t.startsWith(hMarker)) {
        final h = ((jsonDecode(t.substring(hMarker.length)) as Map)['h'] as String?) ?? '';
        return (h.isEmpty || validHash(h)) ? AvatarMsg._('h', h, null) : null;
      }
      if (t.startsWith(qMarker)) {
        final h = ((jsonDecode(t.substring(qMarker.length)) as Map)['h'] as String?) ?? '';
        return validHash(h) ? AvatarMsg._('q', h, null) : null;
      }
      if (t.startsWith(aMarker)) {
        final m = jsonDecode(t.substring(aMarker.length)) as Map;
        final h = (m['h'] as String?) ?? '';
        final b64 = (m['png'] as String?) ?? '';
        if (!validHash(h) || b64.isEmpty || b64.length > maxPng * 4 ~/ 3 + 8) return null;
        final png = Uint8List.fromList(base64.decode(b64));
        return AvatarMsg._('a', h, png);
      }
    } catch (_) {}
    return null;
  }
}
