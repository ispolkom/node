import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';

/// Шифрование кусков файла: AES-256-GCM, свой ключ на каждый файл.
///
/// Кусок на проводе: [12 байт случайное число][шифртекст][16 байт метка]. В проверяемые данные (AAD) входят номер передачи, номер куска
/// и их общее число, поэтому узел не может переставить, подменить или обрезать куски незаметно: расшифровка не пройдёт.
class FileCrypto {
  static final AesGcm _aes = AesGcm.with256bits();

  static Uint8List newKey() {
    final r = Random.secure();
    return Uint8List.fromList(List<int>.generate(32, (_) => r.nextInt(256)));
  }

  static List<int> _aad(String tid, int idx, int total) => utf8.encode('yandi-file:v1|$tid|$idx|$total');

  static Future<Uint8List> encryptChunk(List<int> key, String tid, int idx, int total, List<int> plain) async {
    final box = await _aes.encrypt(plain, secretKey: SecretKey(key), aad: _aad(tid, idx, total));
    return Uint8List.fromList(box.concatenation());
  }

  /// Бросает исключение, если кусок изменён, чужой или стоит не на своём месте.
  static Future<Uint8List> decryptChunk(List<int> key, String tid, int idx, int total, List<int> wire) async {
    final box = SecretBox.fromConcatenation(wire, nonceLength: 12, macLength: 16);
    final plain = await _aes.decrypt(box, secretKey: SecretKey(key), aad: _aad(tid, idx, total));
    return Uint8List.fromList(plain);
  }
}
