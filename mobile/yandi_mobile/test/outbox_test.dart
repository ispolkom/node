import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/contact_card.dart';
import 'package:yandi_mobile/models/file_offer.dart';
import 'package:yandi_mobile/models/group_invite.dart';
import 'package:yandi_mobile/models/message.dart';
import 'package:yandi_mobile/services/outbox.dart';

ChatMessage msg(String id, {String text = 'привет', bool out = true, MessageStatus st = MessageStatus.failed, DateTime? at}) =>
    ChatMessage(id: id, peerId: 'p', outgoing: out, text: text, timestamp: at ?? DateTime(2026, 10, 18, 12), status: st);

void main() {
  final t0 = DateTime(2026, 10, 18, 12, 0, 1);

  test('повторяются только неотправленные исходящие личные тексты не старше суток', () {
    expect(Outbox.retryable(msg('a'), t0), true);
    expect(Outbox.retryable(msg('a', out: false), t0), false, reason: 'входящее');
    expect(Outbox.retryable(msg('a', st: MessageStatus.pending), t0), false, reason: 'ушло, ждёт квитанции');
    expect(Outbox.retryable(msg('a', st: MessageStatus.delivered), t0), false);
    expect(Outbox.retryable(msg('a', text: '  '), t0), false);
    expect(Outbox.retryable(msg('a'), DateTime(2026, 10, 19, 12, 0, 1)), false, reason: 'суток хватит');
    expect(Outbox.retryable(msg('a'), DateTime(2026, 10, 19, 11, 59)), true);
  });

  test('паузы растут 2, 5, 15, 30, 60 секунд, затем раз в пять минут', () {
    final o = Outbox();
    final m = msg('a');
    var now = t0;
    final waits = <int>[];
    for (var i = 0; i < 8; i++) {
      expect(o.due([m], now), [m], reason: 'попытка $i пора');
      o.failedAgain('a', now);
      // сразу после неудачи — ещё рано
      expect(o.due([m], now), isEmpty);
      final w = (i < Outbox.delaysSec.length ? Outbox.delaysSec[i] : Outbox.slowSec);
      waits.add(w);
      expect(o.due([m], now.add(Duration(seconds: w - 1))), isEmpty);
      now = now.add(Duration(seconds: w));
    }
    expect(waits, [2, 5, 15, 30, 60, 300, 300, 300]);
  });

  test('когда связь вернулась, ждущие паузы пробуются сразу, а счётчик попыток не сбрасывается', () {
    final o = Outbox();
    final m = msg('a');
    o.failedAgain('a', t0);
    o.failedAgain('a', t0);
    expect(o.due([m], t0), isEmpty);
    o.connectionBack(t0);
    expect(o.due([m], t0), [m]);
    o.failedAgain('a', t0); // третья неудача: пауза уже 15 с, а не 2
    expect(o.due([m], t0.add(const Duration(seconds: 14))), isEmpty);
    expect(o.due([m], t0.add(const Duration(seconds: 15))), [m]);
  });

  test('ушедшее сообщение забывается, очередь идёт по порядку отправки и не длиннее прохода', () {
    final o = Outbox();
    final list = [
      for (var i = 0; i < 30; i++) msg('m$i', at: DateTime(2026, 10, 18, 12).add(Duration(seconds: 30 - i))),
    ];
    final pass = o.due(list, t0.add(const Duration(minutes: 2)));
    expect(pass.length, Outbox.perPass);
    expect(pass.first.id, 'm29', reason: 'самое раннее — первым');
    o.failedAgain('m29', t0);
    o.sent('m29');
    expect(o.waiting, 0);
    o.failedAgain('x', t0);
    o.keepOnly({'m1'});
    expect(o.waiting, 0, reason: 'неизвестные счётчики стёрты');
  });

  test('файлы, приглашения в группы и карточки контактов сюда не попадают', () {
    final offer = const FileOffer(tid: 't', name: 'a.txt', size: 1, chunks: 1, mime: 'text/plain', key: 'k').encode();
    final invite = GroupInviteMsg.encode('a' * 32, '', 'Семья');
    final card = ContactCardMsg.encode('b' * 16, 'b' * 64, 'Боб');
    for (final t in [offer, invite, card]) {
      expect(Outbox.retryable(msg('a', text: t), t0), false, reason: t);
    }
    // а обычный текст, даже похожий на служебный, повторяется
    expect(Outbox.retryable(msg('a', text: 'yandi-file: привет'), t0), true);
  });

  test('заблокированный собеседник не занимает проход: пропускается до лимита', () {
    final o = Outbox();
    final list = [
      for (var i = 0; i < 30; i++) ChatMessage(id: 'x$i', peerId: 'blocked', outgoing: true, text: 'a$i', timestamp: DateTime(2026, 10, 18, 12).add(Duration(seconds: i)), status: MessageStatus.failed),
      ChatMessage(id: 'ok', peerId: 'friend', outgoing: true, text: 'привет', timestamp: DateTime(2026, 10, 18, 12, 5), status: MessageStatus.failed),
    ];
    final pass = o.due(list, t0.add(const Duration(minutes: 10)), skip: (m) => m.peerId == 'blocked');
    expect(pass.map((m) => m.id), ['ok'], reason: 'очередь друга идёт, хотя 30 сообщений заблокированному старше');
  });

  test('ушло без квитанции: повтор через минуту, 5 минут и полчаса, три раза и всё, только для умеющих квитанции', () {
    final o = Outbox();
    final sentAt = DateTime(2026, 10, 18, 12);
    final m = ChatMessage(id: 'p', peerId: 'friend', outgoing: true, text: 'привет', timestamp: sentAt, status: MessageStatus.pending);
    bool yes(ChatMessage _) => true;
    expect(o.duePending([m], sentAt.add(const Duration(seconds: 59)), capable: yes), isEmpty, reason: 'ещё рано');
    expect(o.duePending([m], sentAt.add(const Duration(seconds: 60)), capable: yes), [m]);
    o.pendingResent('p');
    expect(o.duePending([m], sentAt.add(const Duration(seconds: 299)), capable: yes), isEmpty);
    expect(o.duePending([m], sentAt.add(const Duration(seconds: 300)), capable: yes), [m]);
    o.pendingResent('p');
    expect(o.duePending([m], sentAt.add(const Duration(seconds: 1800)), capable: yes), [m]);
    o.pendingResent('p');
    expect(o.duePending([m], sentAt.add(const Duration(hours: 3)), capable: yes), isEmpty, reason: 'три повтора израсходованы: ящик узла не засоряем');
    // собеседник без квитанций (старый клиент без дедупликации) — не повторяем: он показал бы дубли
    expect(Outbox().duePending([m], sentAt.add(const Duration(minutes: 2)), capable: (_) => false), isEmpty);
    // доставленное и служебное не повторяются; старше суток — тоже
    final done = ChatMessage(id: 'd', peerId: 'friend', outgoing: true, text: 'x', timestamp: sentAt, status: MessageStatus.delivered);
    final file = ChatMessage(id: 'f', peerId: 'friend', outgoing: true, text: FileOffer.marker + '{}', timestamp: sentAt, status: MessageStatus.pending);
    expect(Outbox().duePending([done, file], sentAt.add(const Duration(minutes: 2)), capable: yes), isEmpty);
    expect(Outbox().duePending([m], sentAt.add(const Duration(hours: 25)), capable: yes), isEmpty);
    // квитанция пришла — счётчик забыт
    o.forgetPending({});
    expect(o.duePending([m], sentAt.add(const Duration(seconds: 61)), capable: yes), [m], reason: 'счётчик очищен вместе с исчезнувшим сообщением');
  });
}

