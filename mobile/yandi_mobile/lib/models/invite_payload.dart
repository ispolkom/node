import 'dart:convert';

/// Приглашение нового устройства. Тот же формат, что у кода сопряжения узла (`YANDI-PAIR-1:<base64 JSON>`): его понимает экран сопряжения и
/// сканер QR. Адрес и отпечаток сертификата — те узла, с которым соединено приглашающее устройство. `invite` и `exp` (секунды Unix)
/// говорят принимающему, что нужен PIN и когда приглашение истечёт (узел проверяет срок сам, `exp` — для понятного сообщения).
String inviteText({required String host, required int port, required String code, required String fingerprint, int? expiresAtSecs}) {
  final payload = jsonEncode({
    'host': host,
    'port': port,
    'pairing_code': code,
    'tls_fingerprint': fingerprint,
    'tls': true,
    'invite': true,
    if (expiresAtSecs != null) 'exp': expiresAtSecs,
  });
  return 'YANDI-PAIR-1:${base64.encode(utf8.encode(payload))}';
}

/// Ссылка, по которой приглашение открывается в приложении одним нажатием: `yandi://invite?d=<base64url текста приглашения>`.
String inviteLink(String text) => 'yandi://invite?d=${base64Url.encode(utf8.encode(text)).replaceAll('=', '')}';

/// Из ссылки обратно в текст приглашения; не наша или повреждённая ссылка — null.
String? inviteTextFromLink(String link) {
  final u = Uri.tryParse(link.trim());
  if (u == null || u.scheme != 'yandi' || u.host != 'invite') return null;
  final d = u.queryParameters['d'];
  if (d == null || d.isEmpty || d.length > 4096) return null;
  try {
    final text = utf8.decode(base64Url.decode(base64Url.normalize(d)));
    return text.startsWith('YANDI-PAIR-1:') ? text : null;
  } catch (_) {
    return null;
  }
}

/// Срок приглашения из текста (секунды Unix), если он там есть.
int? inviteExpiryFromText(String text) {
  try {
    final t = text.trim();
    if (!t.startsWith('YANDI-PAIR-1:')) return null;
    final m = jsonDecode(utf8.decode(base64.decode(t.substring('YANDI-PAIR-1:'.length).trim())));
    final e = m is Map ? m['exp'] : null;
    return e is int ? e : null;
  } catch (_) {
    return null;
  }
}
