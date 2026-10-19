/// Файлы, которые приложение готовит к отправке (проверка, шифрование в хранилище): видны в чате сразу, до появления сообщения.
/// Каждая подготовка имеет свой номер: снимается только своя запись; одинаковые имена у разных файлов не мешают друг другу,
/// а повторный выбор того же источника (пути) распознаётся.
class PrepTracker {
  final Map<String, List<({int id, String label, String source})>> _items = {};
  int _seq = 0;

  List<String> labels(String peerId) => [for (final p in _items[peerId] ?? const []) p.label];

  bool hasSource(String peerId, String source) => (_items[peerId] ?? const []).any((p) => p.source == source);

  int start(String peerId, String label, String source) {
    final id = ++_seq;
    _items.putIfAbsent(peerId, () => []).add((id: id, label: label, source: source));
    return id;
  }

  /// true — запись была и снята; повторное снятие и чужой номер ничего не меняют.
  bool done(String peerId, int id) {
    final l = _items[peerId];
    if (l == null) return false;
    final before = l.length;
    l.removeWhere((p) => p.id == id);
    if (l.length == before) return false;
    if (l.isEmpty) _items.remove(peerId);
    return true;
  }
}
