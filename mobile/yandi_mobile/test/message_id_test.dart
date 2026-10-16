import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/message.dart';

void main() {
  test('номера исходящих не совпадают даже в одну миллисекунду', () {
    final ids = {for (var i = 0; i < 5000; i++) ChatMessage.generateId()};
    expect(ids.length, 5000);
  });

  test('номер входящего: то же письмо — тот же номер, другой текст/отправитель/время — другой', () {
    final t = DateTime.fromMillisecondsSinceEpoch(1700000000000);
    final a = ChatMessage.incomingId('aa', t, 'привет');
    expect(ChatMessage.incomingId('aa', t, 'привет'), a);
    expect(ChatMessage.incomingId('aa', t, 'пока'), isNot(a));
    expect(ChatMessage.incomingId('bb', t, 'привет'), isNot(a));
    expect(ChatMessage.incomingId('aa', t.add(const Duration(milliseconds: 1)), 'привет'), isNot(a));
  });
}
