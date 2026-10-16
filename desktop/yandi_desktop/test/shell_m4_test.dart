import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;
import 'package:yandi_desktop/message_actions.dart';
import 'package:yandi_desktop/content_policy.dart';
import 'package:yandi_desktop/voice.dart';

void main() {
  testWidgets('own text edit, local delete and real clear confirmation', (
    t,
  ) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.byTooltip('Отправлено'), findsOneWidget);
    await m1.command(t, '<b>Текст</b>', 'Изменить');
    expect(find.text('Редактирование сообщения'), findsOneWidget);
    await t.enterText(find.widgetWithText(TextField, 'Сообщение'), 'правка');
    await m1.click(t, 'Отправить');
    expect(find.text('Редактирование сообщения'), findsNothing);
    expect(api.methods.contains('PATCH'), true);
    expect(api.bodies.last, {'msg_id': 'new', 'text': 'правка'});
    await m1.command(t, '<b>Текст</b>', 'Удалить');
    await m1.click(t, 'Отмена');
    expect(api.methods.contains('DELETE'), false);
    await m1.command(t, '<b>Текст</b>', 'Удалить');
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.last, {'msg_id': 'new', 'for_everyone': false});
    await m1.tip(t, 'Очистить историю');
    expect(api.calls.where((p) => p.contains('/clear/')), isEmpty);
    await m1.click(t, 'Подтвердить');
    expect(api.calls.where((p) => p.contains('/clear/')).length, 1);
    await m1.stop(t);
  });
  testWidgets('reply sends reply_to and quotes the original', (t) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'r1',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': 'ответная',
      'reply_to': {'msg_id': 'new', 'text': 'исходное'},
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.text('исходное'), findsOneWidget);
    await m1.command(t, '<b>Текст</b>', 'Ответить');
    await t.enterText(find.widgetWithText(TextField, 'Сообщение'), 'ок');
    await m1.click(t, 'Отправить');
    expect(api.bodies.last['reply_to'], {
      'msg_id': 'new',
      'text': '<b>Текст</b>',
    });
    await m1.stop(t);
  });
  testWidgets(
    'foreign messages cannot be edited or deleted; attachment cannot be edited',
    (t) async {
      final api = m2.FeatureApi();
      api.historyRows.first['from'] = m2.alice;
      await m2.open(t, api, 'Чат');
      await m1.click(t, 'Открыть чат');
      await t.tap(find.text('<b>Текст</b>'));
      await t.pumpAndSettle();
      expect(find.text('Ответить'), findsOneWidget);
      expect(find.text('Изменить'), findsNothing);
      expect(find.text('Удалить'), findsNothing);
      await m1.stop(t);
    },
  );
  testWidgets('select several messages and delete them together', (t) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'second',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': 'вторая',
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await m1.command(t, '<b>Текст</b>', 'Выбрать');
    expect(find.text('Выбрано: 1'), findsOneWidget);
    await t.tap(find.text('вторая'));
    await t.pumpAndSettle();
    expect(find.text('Выбрано: 2'), findsOneWidget);
    await m1.word(t, 'Удалить выбранные');
    await m1.click(t, 'Подтвердить');
    expect(api.methods.where((m) => m == 'DELETE').length, 2);
    await m1.stop(t);
  });
  testWidgets('chat header has call words and a group invite is a card', (
    t,
  ) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'inv',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text':
          '\u0001yandi-gi:{"g":"${'a' * 32}","c":"abcdefgh12345678","n":"Клуб"}',
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.byTooltip('Позвонить'), findsOneWidget);
    expect(find.byTooltip('Видеозвонок'), findsOneWidget);
    expect(t.widget<Tooltip>(find.byTooltip('Позвонить')).waitDuration, const Duration(seconds: 2)); // подсказка — после 2 с наведения
    expect(find.text('Очистить историю'), findsNothing);   // слов-ссылок в шапке чата больше нет
    expect(find.textContaining('«Клуб»'), findsOneWidget);
    expect(find.text('Вступить в группу'), findsWidgets);
    await m1.stop(t);
  });
  test('карточка контакта: разбор отбрасывает чужое и чистит имя', () {
    final ok = ContactCardMessage.tryParse(ContactCardMessage.encode('d' * 16, 'd' * 64, 'Кор\u0007'));
    expect(ok?.shortId, 'd' * 16);
    expect(ok?.fullId, 'd' * 64);
    expect(ok?.name, 'Кор');
    expect(ContactCardMessage.tryParse('\u0001yandi-ct:{"s":"zz"}'), isNull);
    expect(ContactCardMessage.tryParse('\u0001yandi-ct:не json'), isNull);
    expect(ContactCardMessage.tryParse('обычный текст'), isNull);
    // полный ID, не начинающийся с короткого, отбрасывается — остаётся только короткий
    final mismatch = ContactCardMessage.tryParse('\u0001yandi-ct:{"s":"${'d' * 16}","i":"${'e' * 64}","n":"x"}');
    expect(mismatch?.fullId, '');
  });
  testWidgets('поделиться контактом: выбор, подтверждение, карточка уходит в личный чат', (t) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await m1.click(t, 'Поделиться контактом');
    expect(find.text('Алиса'), findsWidgets);
    expect(find.byKey(const ValueKey('share-0')), findsOneWidget);
    expect(find.byKey(const ValueKey('share-1')), findsNothing);   // собеседнику не предлагают его самого
    await t.tap(find.byKey(const ValueKey('share-0')));
    await t.pumpAndSettle();
    await m1.click(t, 'ОК');
    final sent = api.bodies.where((b) => '${b['text'] ?? ''}'.startsWith(ContactCardMessage.marker)).toList();
    expect(sent.length, 1);
    final card = ContactCardMessage.tryParse('${sent.first['text']}')!;
    expect(card.shortId, 'b' * 16);
    expect(card.name, 'Боб');
    await m1.stop(t);
  });
  testWidgets('полученный контакт: карточка и «Сохранить» с изменением имени только у себя', (t) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'ct',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': ContactCardMessage.encode('d' * 16, '', 'Кор'),
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.text('Кор'), findsOneWidget);
    expect(find.text('Контакт от собеседника'), findsOneWidget);
    await m1.tip(t, 'Сохранить контакт');
    await t.enterText(find.byType(TextField).last, 'Вася');
    await m1.click(t, 'Сохранить');
    expect(api.bodies.any((b) => b['short_id'] == 'd' * 16 && b['name'] == 'Вася'), true);
    expect(cleanPersonName('Корhttps://example.org/x'), 'Кор');   // ссылка из буфера в имя не попадает
    await m1.stop(t);
  });
  testWidgets('контакт: «Картинка» есть у каждого, «Как он сам» не показывается, пока ничего локального не задано', (t) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    expect(find.byKey(ValueKey('avatar-${'a' * 16}')), findsOneWidget);
    expect(find.byKey(ValueKey('avatar-${'b' * 16}')), findsOneWidget);
    expect(find.byKey(ValueKey('reset-local-${'a' * 16}')), findsNothing);
    await m1.stop(t);
  });
  testWidgets('emoji panel inserts into the composer in chat', (t) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.enterText(find.widgetWithText(TextField, 'Сообщение'), 'ку');
    await t.tap(find.byTooltip('Эмодзи'));
    await t.pumpAndSettle();
    await t.tap(find.text('😀'));
    await t.pumpAndSettle();
    final field = t.widget<TextField>(find.widgetWithText(TextField, 'Сообщение'));
    expect(field.controller!.text.endsWith('😀'), true);
    expect(field.controller!.text.startsWith('ку'), true);
    await m1.stop(t);
  });
  testWidgets('чат: переслать и выбор, файлы чата без сообщений, голосовое', (t) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'lnk',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': 'смотри https://example.org/page',
    });
    api.historyRows.add({
      'msg_id': 'pic',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch + 1,
      'text': '',
      'attachment': {
        'filename': 'кот.png',
        'size': 10,
        'file_ref': {'file_id': 'f1', 'total_chunks': 1},
      },
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    // удержание/нажатие: «Переслать» и «Выбрать» есть, выбор показывает счётчик и «Переслать выбранные»
    await m1.command(t, '<b>Текст</b>', 'Выбрать');
    expect(find.text('Выбрано: 1'), findsOneWidget);
    expect(find.text('Переслать выбранные'), findsOneWidget);
    await m1.click(t, 'Переслать выбранные');
    expect(find.text('Переслать сообщение'), findsOneWidget);
    expect(find.text('Боб'), findsOneWidget);
    await t.tap(find.text('Отмена').last);
    await t.pumpAndSettle();
    await m1.click(t, 'Снять выбор');
    // «Файлы чата»: сообщений нет, разделы по видам, ссылки отдельно
    await m1.tip(t, 'Файлы чата');
    expect(find.text('<b>Текст</b>'), findsNothing);
    expect(find.text('https://example.org/page'), findsOneWidget);
    expect(find.text('кот.png'), findsOneWidget);
    await m1.word(t, 'Картинки');
    expect(find.text('https://example.org/page'), findsNothing);
    expect(find.text('кот.png'), findsOneWidget);
    await m1.word(t, 'Ссылки');
    expect(find.text('кот.png'), findsNothing);
    expect(find.text('https://example.org/page'), findsOneWidget);
    await m1.tip(t, 'Назад к сообщениям');
    expect(find.text('<b>Текст</b>'), findsOneWidget);
    await m1.stop(t);
  });

  testWidgets('чат: слово «Смайл…» открывает набор и отправляет выбранный смайл', (t) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await m1.command(t, '<b>Текст</b>', 'Смайл…');
    await t.tap(find.byKey(const ValueKey('rx-😊')));
    await t.pumpAndSettle();
    expect(api.calls.any((p) => p.startsWith('/api/chat/react/')), true);
    expect(api.bodies.any((b) => b['emoji'] == '😊' && b['msg_id'] == 'new'), true);
    await m1.stop(t);
  });

  testWidgets('чат: микрофон вместо самолётика, запись и отмена', (t) async {
    final old = voiceBackendFactory;
    voiceBackendFactory = _FakeVoice.new;
    voicePathFactory = () async => '/tmp/yandi_voice_test/Голосовое_test.m4a';
    addTearDown(() {
      voiceBackendFactory = old;
      voicePathFactory = newVoicePath;
    });
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.byKey(const ValueKey('voice-mic')), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('voice-mic')));
    await t.pump(const Duration(seconds: 1));
    expect(find.text('Идёт запись…'), findsOneWidget);
    await t.tap(find.text('Отмена').first);
    await t.pumpAndSettle();
    expect(find.text('Идёт запись…'), findsNothing);
    await t.enterText(find.widgetWithText(TextField, 'Сообщение'), 'привет');
    await t.pump();
    expect(find.byKey(const ValueKey('send-text')), findsOneWidget);
    await m1.stop(t);
  });
}

class _FakeVoice implements VoiceBackend {
  @override
  Future<bool> hasPermission() async => true;
  @override
  Future<void> start(String path) async {}
  @override
  Future<String?> stop() async => null;
  @override
  Future<void> dispose() async {}
}
