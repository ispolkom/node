import 'dart:convert';

class PeerKeyBundle {
  final List<int> x25519, ed25519;
  PeerKeyBundle(this.x25519, this.ed25519);
}

/// TOFU is not external identity verification. A changed signing key is never accepted silently.
class PeerTrust {
  final Future<String?> Function(String) read;
  final Future<void> Function(String, String) write;
  PeerTrust({required this.read, required this.write});
  static Future<void> _tail = Future.value();

  Future<void> accept(String peer, List<int> ed) async {
    final operation = _tail.then((_) async {
      final key = 'peer_ed25519_pin_$peer';
      final encoded = base64.encode(ed);
      final old = await read(key);
      if (old != null && old != encoded) {
        throw StateError(
            'Ключ собеседника $peer изменился. Отправка запрещена: проверьте отпечаток вне YANDI.');
      }
      if (old == null) await write(key, encoded);
    });
    _tail = operation.then<void>((_) {}, onError: (Object _) {});
    await operation;
  }
}
