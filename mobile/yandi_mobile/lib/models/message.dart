import 'dart:convert';
import 'dart:math';
import 'package:crypto/crypto.dart';

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

  /// Ответ-цитата: id цитируемого сообщения (как его знает собеседник) и выдержка. Хранятся в БД.
  final String?       replyId;
  final String?       replyText;

  const ChatMessage({
    required this.id,
    required this.peerId,
    required this.outgoing,
    required this.text,
    required this.timestamp,
    this.status = MessageStatus.pending,
    this.remoteId,
    this.replyId,
    this.replyText,
  });

  /// Номер нового исходящего сообщения: время + 64 случайных бита (два сообщения в одну миллисекунду не совпадают).
  static String generateId() {
    final rng = Random.secure();
    final ts  = DateTime.now().millisecondsSinceEpoch.toRadixString(16);
    final rand = List.generate(8, (_) => rng.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
    return '$ts-$rand';
  }

  /// Номер входящего из E4-конверта: время, отправитель и отпечаток текста. Одно и то же письмо даёт один номер (повтор безопасен),
  /// разные письма одного времени или от разных собеседников — разные.
  static String incomingId(String fromPeerId, DateTime timestamp, String text) {
    final digest = sha256.convert(utf8.encode('$fromPeerId|${timestamp.millisecondsSinceEpoch}|$text')).toString().substring(0, 20);
    return 'inbox_${timestamp.millisecondsSinceEpoch}_$digest';
  }

  ChatMessage copyWith({MessageStatus? status, String? remoteId}) => ChatMessage(
    id:        id,
    peerId:    peerId,
    outgoing:  outgoing,
    text:      text,
    timestamp: timestamp,
    status:    status ?? this.status,
    remoteId:  remoteId ?? this.remoteId,
    replyId:   replyId,
    replyText: replyText,
  );
}
