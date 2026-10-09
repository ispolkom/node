import 'dart:math';

enum MessageStatus { pending, delivered, read, failed }

class ChatMessage {
  final String        id;
  final String        peerId;
  final bool          outgoing;
  final String        text;
  final DateTime      timestamp;
  final MessageStatus status;

  /// id этого сообщения у ОТПРАВИТЕЛЯ (из конверта), чтобы прислать ему квитанцию «прочитано».
  /// Только для входящих и только в памяти (в БД не хранится): квитанции — вещь текущей сессии.
  final String?       remoteId;

  const ChatMessage({
    required this.id,
    required this.peerId,
    required this.outgoing,
    required this.text,
    required this.timestamp,
    this.status = MessageStatus.pending,
    this.remoteId,
  });

  static String generateId() {
    final rng = Random();
    final ts  = DateTime.now().millisecondsSinceEpoch.toRadixString(16);
    final rand = rng.nextInt(0xFFFFFF).toRadixString(16).padLeft(6, '0');
    return '$ts-$rand';
  }

  ChatMessage copyWith({MessageStatus? status}) => ChatMessage(
    id:        id,
    peerId:    peerId,
    outgoing:  outgoing,
    text:      text,
    timestamp: timestamp,
    status:    status ?? this.status,
    remoteId:  remoteId,
  );
}
