import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/receipt.dart';

void main() {
  test('реакция личного чата: конверт разбирается, чужие смайлы и мусор отвергаются, служебное не показывается текстом', () {
    final t = MsgChannel.reaction('abc123', '👍');
    expect(MsgChannel.isService(t), isTrue);
    expect(MsgChannel.parseReaction(t), ('abc123', '👍'));
    expect(MsgChannel.parseReaction(MsgChannel.reaction('abc123', '')), ('abc123', ''), reason: 'пустой смайл — снять реакцию');
    expect(MsgChannel.parseReaction(MsgChannel.reaction('abc123', '🦄')), isNull, reason: 'смайла нет в наборе');
    expect(MsgChannel.parseReaction('\u0001yandi-rx:не json'), isNull);
    expect(MsgChannel.parseReaction(MsgChannel.reaction('', '👍')), isNull);
    expect(MsgChannel.parseReaction('привет'), isNull);
  });
}
