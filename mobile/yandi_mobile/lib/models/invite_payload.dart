import 'dart:convert';

/// Текст приглашения нового устройства, тот же формат, что у кода сопряжения узла (`YANDI-PAIR-1:<base64 JSON>`): его понимает экран
/// сопряжения и сканер QR. Адрес и отпечаток сертификата — те узла, с которым соединено приглашающее устройство.
String inviteText({required String host, required int port, required String code, required String fingerprint}) {
  final payload = jsonEncode({'host': host, 'port': port, 'pairing_code': code, 'tls_fingerprint': fingerprint, 'tls': true});
  return 'YANDI-PAIR-1:${base64.encode(utf8.encode(payload))}';
}
