import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/message.dart';
import 'package:yandi_mobile/models/receipt.dart';

void main() {
  test('конверт сообщения несёт ответ-цитату и читается обратно', () {
    final wire = MsgChannel.wrapMessage('42', 'ответ', replyId: 'abc', replyText: 'было');
    final p = MsgChannel.parseMessageFull(wire)!;
    expect(p.$1, '42');
    expect(p.$2, 'ответ');
    expect(p.$3, 'abc');
    expect(p.$4, 'было');
    // старый разбор тоже работает: цитата ему не мешает
    expect(MsgChannel.parseMessage(wire), ('42', 'ответ'));
  });

  test('без цитаты поля ответа пустые, длинная выдержка режется до 200 знаков', () {
    final plain = MsgChannel.parseMessageFull(MsgChannel.wrapMessage('1', 'привет'))!;
    expect(plain.$3, isNull);
    expect(plain.$4, isNull);
    final long = MsgChannel.parseMessageFull(MsgChannel.wrapMessage('2', 'x', replyId: 'i', replyText: 'я' * 500))!;
    expect(long.$4!.runes.length, 200);
  });

  test('ответ не ломает копирование сообщения и сохраняется в модели', () {
    final m = ChatMessage(
      id: '1', peerId: 'p', outgoing: true, text: 't', timestamp: DateTime(2026),
      replyId: 'r', replyText: 'q',
    );
    final c = m.copyWith(status: MessageStatus.delivered);
    expect(c.replyId, 'r');
    expect(c.replyText, 'q');
  });
}
