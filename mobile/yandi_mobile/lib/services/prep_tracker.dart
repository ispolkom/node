import 'dart:async';

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

/// Владение временной копией выбранного файла в кеше: пока хоть одна операция её читает, затирать нельзя; стирает последний, кто отпустил.
/// Стирание и новый читатель не пересекаются: затирание занимает путь (`beginWipe`) синхронно, а новый читатель ждёт его окончания.
class CopyOwners {
  final Map<String, int> _users = {};
  final Map<String, Completer<void>> _wiping = {};

  /// Занять копию читателем. Если её сейчас затирают — дождаться конца затирания (копии к тому времени уже нет, чтение честно
  /// закончится ошибкой «нет файла», а не чтением наполовину затёртого).
  Future<void> acquire(String path) async {
    while (_wiping.containsKey(path)) {
      await _wiping[path]!.future;
    }
    _users[path] = (_users[path] ?? 0) + 1;
  }

  /// true — читателей не осталось, копию можно стирать.
  bool release(String path) {
    final n = (_users[path] ?? 0) - 1;
    if (n > 0) {
      _users[path] = n;
      return false;
    }
    _users.remove(path);
    return true;
  }

  bool inUse(String path) => (_users[path] ?? 0) > 0;

  /// Начать затирание: false, если копию читают или её уже затирают. Вызывается синхронно перед самим затиранием.
  bool beginWipe(String path) {
    if (inUse(path) || _wiping.containsKey(path)) return false;
    _wiping[path] = Completer<void>();
    return true;
  }

  void endWipe(String path) {
    _wiping.remove(path)?.complete();
  }
}
