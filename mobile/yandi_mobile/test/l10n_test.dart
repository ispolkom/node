import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/l10n.dart';
import 'package:yandi_mobile/l10n_en.dart';
import 'package:yandi_mobile/models/guest_node.dart';
import 'package:yandi_mobile/models/support_wallet.dart';
import 'package:yandi_mobile/screens/guest_screen.dart';
import 'package:yandi_mobile/screens/invite_device_screen.dart';
import 'package:yandi_mobile/screens/support_screen.dart';
import 'package:yandi_mobile/models/invite_payload.dart';

/// Ключи `tr('…')`/`trf('…')` из исходников: склеенные соседние литералы, как их видит компилятор.
Set<String> keysIn(String src) {
  final out = <String>{};
  final call = RegExp(r"\btrf?\(((?:\s*'(?:[^'\\]|\\.)*'\s*)+)");
  for (final m in call.allMatches(src)) {
    final body = RegExp(r"'((?:[^'\\]|\\.)*)'").allMatches(m.group(1)!).map((x) => x.group(1)!).join().replaceAll("\\'", "'").replaceAll('\\n', '\n').replaceAll('\\\$', '\$');
    out.add(body);
  }
  return out;
}

void main() {
  test('у каждой надписи в коде есть английский перевод, подстановки {имя} совпадают', () {
    final keys = <String>{};
    for (final f in Directory('lib').listSync(recursive: true).whereType<File>().where((f) => f.path.endsWith('.dart') && !f.path.endsWith('l10n_en.dart') && !f.path.endsWith('l10n.dart') && !f.path.endsWith('l10n_core.dart'))) {
      keys.addAll(keysIn(f.readAsStringSync()));
    }
    expect(keys, isNotEmpty);
    final missing = keys.where((k) => !enTable.containsKey(k)).toList();
    expect(missing, isEmpty, reason: 'нет перевода: ${missing.take(5)}');
    final ph = RegExp(r'\{[a-z]+\}');
    for (final k in keys) {
      final a = ph.allMatches(k).map((m) => m.group(0)).toSet();
      final b = ph.allMatches(enTable[k]!).map((m) => m.group(0)).toSet();
      expect(b, a, reason: 'подстановки в переводе: $k');
    }
  });

  test('в таблице нет лишних ключей и пустых переводов', () {
    final keys = <String>{};
    for (final f in Directory('lib').listSync(recursive: true).whereType<File>().where((f) => f.path.endsWith('.dart') && !f.path.endsWith('l10n_en.dart') && !f.path.endsWith('l10n.dart') && !f.path.endsWith('l10n_core.dart'))) {
      keys.addAll(keysIn(f.readAsStringSync()));
    }
    // команды меню сообщения показываются через tr(команда), их ключи берутся из констант, а не из tr('…')
    final cmds = RegExp(r"cmd\w+ = '([^']+)'").allMatches(File('lib/widgets/message_menu.dart').readAsStringSync()).map((m) => m.group(1)!).toSet();
    final extra = enTable.keys.where((k) => !keys.contains(k) && !cmds.contains(k)).toList();
    expect(extra, isEmpty, reason: 'лишние ключи: ${extra.take(5)}');
    expect(enTable.values.every((v) => v.trim().isNotEmpty), isTrue);
  });

  test('команды меню сообщения: значение — русская команда, у каждой есть английская надпись', () {
    // значения команд — идентификаторы (по ним переключаются экраны), показываются через tr(команда)
    final src = File('lib/widgets/message_menu.dart').readAsStringSync();
    final cmds = RegExp(r"cmd\w+ = '([^']+)'").allMatches(src).map((m) => m.group(1)!).toList();
    expect(cmds.length, greaterThanOrEqualTo(11));
    for (final c in cmds) {
      expect(enTable.containsKey(c), isTrue, reason: 'нет перевода команды: $c');
    }
  });

  test('tr и trf: русский по умолчанию в тестах, английский после переключения; подстановки', () {
    expect(tr('Отмена'), 'Отмена');
    expect(trf('Слово {n}', {'n': 3}), 'Слово 3');
    Lang.forTests(AppLang.en);
    addTearDown(() => Lang.forTests(AppLang.ru));
    expect(tr('Отмена'), 'Cancel');
    expect(trf('Слово {n}', {'n': 3}), 'Word 3');
    expect(tr('нет такого ключа'), 'нет такого ключа', reason: 'без перевода остаётся русский');
  });

  testWidgets('переключатель RU|EN меняет надписи на экране и запоминается как выбранный', (t) async {
    addTearDown(() => Lang.forTests(AppLang.ru));
    await t.pumpWidget(MaterialApp(home: Scaffold(body: Column(children: [const LangToggle(), Builder(builder: (_) => Text(tr('Отмена')))]))));
    expect(find.text('Отмена'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('lang-en')));
    await t.pump();
    expect(Lang.current.value, AppLang.en);
    expect(find.text('Cancel'), findsOneWidget, reason: 'надпись перерисована на английском');
    await t.tap(find.byKey(const ValueKey('lang-ru')));
    await t.pump();
    expect(find.text('Отмена'), findsOneWidget);
  });

  testWidgets('по-английски на экранах нет русских надписей (поддержка, приглашение, гость)', (t) async {
    Lang.forTests(AppLang.en);
    addTearDown(() => Lang.forTests(AppLang.ru));
    t.view.physicalSize = const Size(900, 3000);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);
    final cyr = RegExp(r'[А-Яа-яЁё]');
    List<String> russian() => [
          for (final w in find.byType(Text, skipOffstage: false).evaluate().map((e) => e.widget as Text)) if (w.data != null && cyr.hasMatch(w.data!)) w.data!,
          for (final w in find.byType(SelectableText, skipOffstage: false).evaluate().map((e) => e.widget as SelectableText)) if (w.data != null && cyr.hasMatch(w.data!)) w.data!,
        ];
    await t.pumpWidget(const MaterialApp(home: SupportScreen()));
    expect(russian(), isEmpty, reason: 'экран поддержки');
    final tx = inviteText(host: 'h', port: 1, code: '11112222', fingerprint: 'cd' * 32);
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        key: const ValueKey('inv'),
        create: () async => throw StateError('x'),
        revoke: () async {},
        restore: () async => (text: tx, link: inviteLink(tx), code: '11112222', pin: '0042', ttlSecs: 1800),
      ),
    ));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(russian(), isEmpty, reason: 'экран приглашения');
    await t.pumpWidget(MaterialApp(home: Scaffold(body: GuestPanel(key: const ValueKey('g'), nodes: [GuestNode('Node', '203.0.113.7', 8444, 'ab' * 32)]))));
    await t.pump();
    expect(russian(), isEmpty, reason: 'экран гостя');
    expect(supportWallets, isNotEmpty);
  });
}
