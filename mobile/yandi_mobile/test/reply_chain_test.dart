import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/group.dart';
import 'package:yandi_mobile/screens/group_chat_screen.dart';

GroupEntry _e(int seq, String from, String t, {int? reply, String? quoted, String? quotedFrom}) => GroupEntry(
      gid: 'g', seq: seq, ts: seq, epoch: 1, kind: 1, from: from,
      body: {'t': t, if (reply != null) 'r': {'s': reply, 'a': quotedFrom ?? '', 't': quoted ?? ''}},
    );

void main() {
  String ex(GroupEntry e) => '${e.body['t']}';
  test('цепочка ответов ведёт от первого сообщения к последнему', () {
    final all = [
      _e(1, 'x', 'Я построил вечный двигатель'),
      _e(2, 'y', 'Из каких материалов?', reply: 1, quoted: 'Я построил вечный двигатель', quotedFrom: 'x'),
      _e(3, 'z', 'Сколько энергии?', reply: 2, quoted: 'Из каких материалов?', quotedFrom: 'y'),
      _e(4, 'w', 'Прототипы есть?', reply: 3, quoted: 'Сколько энергии?', quotedFrom: 'z'),
    ];
    final c = replyChain(all, all.last, (f) => f.toUpperCase(), ex);
    expect(c.map((i) => i.seq), [1, 2, 3, 4]);
    expect(c.first.text, 'Я построил вечный двигатель');
    expect(c.first.author, 'X');
  });

  test('стёртый родитель: цепочка начинается с выдержки из карточки; петля не зацикливает', () {
    final all = [
      _e(3, 'z', 'Сколько энергии?', reply: 2, quoted: 'Из каких материалов?', quotedFrom: 'y'),
      _e(4, 'w', 'Прототипы есть?', reply: 3),
    ];
    final c = replyChain(all, all.last, (f) => f, ex);
    expect(c.map((i) => i.seq), [null, 3, 4]);
    expect(c.first.known, false);
    expect(c.first.text, 'Из каких материалов?');
    final loop = [_e(1, 'a', 'один', reply: 2), _e(2, 'b', 'два', reply: 1)];
    expect(replyChain(loop, loop.first, (f) => f, ex).length, 2);
  });
}
