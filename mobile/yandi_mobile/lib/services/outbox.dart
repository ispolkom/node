import '../models/contact_card.dart';
import '../models/file_offer.dart';
import '../models/group_invite.dart';
import '../models/message.dart';

/// Автоповтор неотправленных личных сообщений (ROADMAP12, шаг 0 «стабильная связь»).
///
/// Сообщение, которое не ушло (нет сокета, нет ключа получателя), раньше оставалось «не отправлено» навсегда. Теперь оно повторяется с
/// нарастающими паузами, тем же номером (получатель отбрасывает повтор по этому номеру), пока не уйдёт или не пройдёт сутки.
/// Только личные текстовые сообщения: передачи файлов, приглашения в группы и карточки контактов повторяют свои собственные пути.
class Outbox {
  /// Паузы между попытками одного сообщения, секунды; после последней — [slowSec].
  static const List<int> delaysSec = [2, 5, 15, 30, 60];
  static const int slowSec = 300;
  static const Duration maxAge = Duration(hours: 24);
  /// Сколько сообщений за один проход (чтобы длинная очередь не забивала сокет).
  static const int perPass = 20;

  /// Повторы «ушло, а квитанции нет»: спустя минуту, 5 минут и полчаса после отправки, не больше трёх раз на сообщение.
  static const List<int> pendingDelaysSec = [60, 300, 1800];

  final Map<String, _Try> _tries = {};
  final Map<String, int> _pendingTries = {};

  /// Можно ли повторять это сообщение.
  static bool retryable(ChatMessage m, DateTime now) {
    if (!m.outgoing || m.status != MessageStatus.failed) return false;
    if (now.difference(m.timestamp) >= maxAge) return false;
    return retryableText(m);
  }

  /// Личный текст (не файл, не приглашение, не карточка).
  static bool retryableText(ChatMessage m) {
    if (m.text.trim().isEmpty) return false;
    // по метке, а не по разбору: повреждённый служебный конверт тоже не должен уйти как обычный текст
    return !FileOffer.isOffer(m.text) && !m.text.startsWith(GroupInviteMsg.marker) && !m.text.startsWith(ContactCardMsg.marker);
  }

  /// Какие из неотправленных пора повторить сейчас: по порядку отправки, не больше [perPass]. [skip] (например, заблокированный
  /// собеседник) отсекается ДО лимита прохода, иначе такие сообщения занимали бы весь проход.
  List<ChatMessage> due(List<ChatMessage> failed, DateTime now, {bool Function(ChatMessage)? skip}) {
    final mine = failed.where((m) => retryable(m, now) && !(skip?.call(m) ?? false)).toList()..sort((a, b) => a.timestamp.compareTo(b.timestamp));
    final out = <ChatMessage>[];
    for (final m in mine) {
      final t = _tries[m.id];
      if (t == null || !now.isBefore(t.nextAt)) out.add(m);
      if (out.length >= perPass) break;
    }
    return out;
  }

  /// Сообщение ушло.
  void sent(String id) => _tries.remove(id);

  /// Ушло в сокет, но подтверждения нет: пакет мог пропасть при обрыве, пока сокет принимал его в буфер. Повтор тем же номером безопасен
  /// ТОЛЬКО для собеседников, умеющих квитанции и отбрасывающих повтор по номеру (их знает вызывающий через [capable]).
  List<ChatMessage> duePending(List<ChatMessage> pending, DateTime now, {required bool Function(ChatMessage) capable}) {
    final out = <ChatMessage>[];
    final sorted = pending.where((m) => m.outgoing && m.status == MessageStatus.pending && now.difference(m.timestamp) < maxAge && capable(m) && retryableText(m)).toList()
      ..sort((a, b) => a.timestamp.compareTo(b.timestamp));
    for (final m in sorted) {
      final n = _pendingTries[m.id] ?? 0;
      if (n >= pendingDelaysSec.length) continue;
      if (now.difference(m.timestamp) >= Duration(seconds: pendingDelaysSec[n])) out.add(m);
      if (out.length >= perPass) break;
    }
    return out;
  }

  /// Повтор отправлен: следующий — позже (или никогда, если все три израсходованы).
  void pendingResent(String id) => _pendingTries[id] = (_pendingTries[id] ?? 0) + 1;

  /// Квитанция пришла или сообщение исчезло: счётчик не нужен.
  void forgetPending(Set<String> stillPending) => _pendingTries.removeWhere((k, _) => !stillPending.contains(k));

  /// Повтор не удался: следующая попытка позже.
  void failedAgain(String id, DateTime now) {
    final t = _tries.putIfAbsent(id, () => _Try());
    final i = t.count < delaysSec.length ? delaysSec[t.count] : slowSec;
    t.count++;
    t.nextAt = now.add(Duration(seconds: i));
  }

  /// Связь вернулась: всё, что ждало паузы, пробуем сразу (счётчик попыток сохраняется, чтобы не молотить при плохой связи).
  void connectionBack(DateTime now) {
    for (final t in _tries.values) {
      t.nextAt = now;
    }
  }

  /// Сколько сообщений сейчас ждёт следующей попытки (для показателей стабильности).
  int get waiting => _tries.length;

  /// Забыть счётчики сообщений, которых больше нет (удалены, ушли, устарели).
  void keepOnly(Set<String> ids) => _tries.removeWhere((k, _) => !ids.contains(k));
}

class _Try {
  int count = 0;
  DateTime nextAt = DateTime.fromMillisecondsSinceEpoch(0);
}
