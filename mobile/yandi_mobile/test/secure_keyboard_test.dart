import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/theme.dart';
import 'package:yandi_mobile/widgets/secure_keyboard.dart';

void main() {
  late TextEditingController c;
  setUp(() => c = TextEditingController());
  tearDown(() => c.dispose());

  Future<void> pump(WidgetTester t, {VoidCallback? onEnter}) async {
    await t.binding.setSurfaceSize(const Size(400, 800));
    await t.pumpWidget(MaterialApp(theme: AppTheme.dark, home: Scaffold(body: Align(alignment: Alignment.bottomCenter, child: SecureKeyboard(controller: c, onEnter: onEnter)))));
  }

  Future<void> tapKey(WidgetTester t, String label) async {
    await t.tap(find.text(label).first);
    await t.pump();
  }

  Future<void> tapSem(WidgetTester t, String semantics) async {
    await t.tap(find.byWidgetPredicate((w) => w is Semantics && w.properties.label == semantics).first);
    await t.pump();
  }

  testWidgets('русская раскладка по умолчанию; заглавная на одну букву, затем снова строчные', (t) async {
    await pump(t);
    await tapKey(t, 'п'); await tapKey(t, 'р');
    await t.tap(find.byIcon(Icons.keyboard_arrow_up)); await t.pump();
    await tapKey(t, 'П');   // после нажатия «⇧» буквы заглавные
    await tapKey(t, 'и');
    expect(c.text, 'прПи');
  });

  testWidgets('смена языка: английская раскладка, пробел', (t) async {
    await pump(t);
    await t.tap(find.text('RU')); await t.pump();
    await tapKey(t, 'h'); await tapKey(t, 'i'); await tapSem(t, 'Пробел'); await tapKey(t, 'y');
    expect(c.text, 'hi y');
    await t.tap(find.text('EN')); await t.pump();   // вернуть русскую для других тестов (раскладка помнится)
  });

  testWidgets('цифры и знаки', (t) async {
    await pump(t);
    await tapSem(t, 'Цифры и знаки');
    await tapKey(t, '1'); await tapKey(t, '2'); await tapKey(t, '₽');
    expect(c.text, '12₽');
    await tapSem(t, 'Буквы');
    await tapKey(t, 'а');
    expect(c.text, '12₽а');
  });

  testWidgets('вставка в позицию курсора и замена выделенного', (t) async {
    await pump(t);
    c.value = const TextEditingValue(text: 'ac', selection: TextSelection.collapsed(offset: 1));
    await tapKey(t, 'б');
    expect(c.text, 'аб'.replaceFirst('а', 'a') + 'c');   // 'a' + 'б' + 'c'
    expect(c.selection.baseOffset, 2);
    c.value = const TextEditingValue(text: 'abcd', selection: TextSelection(baseOffset: 1, extentOffset: 3));
    await tapKey(t, 'я');
    expect(c.text, 'aяd');
  });

  testWidgets('удаление: по одному символу перед курсором, выделенное целиком, пустое — без ошибок', (t) async {
    await pump(t);
    c.value = const TextEditingValue(text: 'abc', selection: TextSelection.collapsed(offset: 3));
    await t.tap(find.byIcon(Icons.backspace_outlined)); await t.pump();
    expect(c.text, 'ab');
    c.value = const TextEditingValue(text: 'abc', selection: TextSelection(baseOffset: 0, extentOffset: 2));
    await t.tap(find.byIcon(Icons.backspace_outlined)); await t.pump();
    expect(c.text, 'c');
    c.value = const TextEditingValue(text: '', selection: TextSelection.collapsed(offset: 0));
    await t.tap(find.byIcon(Icons.backspace_outlined)); await t.pump();
    expect(c.text, '');
  });

  testWidgets('эмодзи: панель вставляет, а удаление убирает сложный эмодзи (семья, флаг, с оттенком кожи) одним нажатием', (t) async {
    await pump(t);
    await tapSem(t, 'Эмодзи');
    await t.tap(find.text(emojiPanel.first)); await t.pump();
    expect(c.text, emojiPanel.first);
    for (final complex in ['👨‍👩‍👧', '🇷🇺', '👍🏽', '❤️']) {
      c.value = TextEditingValue(text: 'a$complex', selection: TextSelection.collapsed(offset: 1 + complex.length));
      await t.tap(find.byIcon(Icons.backspace_outlined)); await t.pump();
      expect(c.text, 'a', reason: complex);
    }
  });

  testWidgets('«Ввод» вставляет перенос строки или вызывает свой обработчик', (t) async {
    await pump(t);
    await t.tap(find.byIcon(Icons.keyboard_return)); await t.pump();
    expect(c.text, '\n');
    var entered = 0;
    c.clear();
    await pump(t, onEnter: () => entered++);
    await t.tap(find.byIcon(Icons.keyboard_return)); await t.pump();
    expect(entered, 1);
    expect(c.text, '');
  });

  testWidgets('удержание «стереть» повторяется', (t) async {
    await pump(t);
    c.value = const TextEditingValue(text: 'abcdefgh', selection: TextSelection.collapsed(offset: 8));
    final g = await t.startGesture(t.getCenter(find.byIcon(Icons.backspace_outlined)));
    await t.pump(const Duration(milliseconds: 700));
    await t.pump(const Duration(milliseconds: 500));
    await g.up();
    await t.pump();
    expect(c.text.length, lessThan(6), reason: 'за секунду должно стереться несколько символов');
  });

  testWidgets('поле с этой клавиатурой не вызывает системную клавиатуру, обычное поле — вызывает', (t) async {
    final own = TextEditingController(), normal = TextEditingController();
    await t.pumpWidget(MaterialApp(home: Scaffold(body: Column(children: [
      TextField(key: const Key('own'), controller: own, keyboardType: SecureKeyboard.inputType, enableSuggestions: false, autocorrect: false, enableIMEPersonalizedLearning: false),
      TextField(key: const Key('normal'), controller: normal),
    ]))));
    await t.tap(find.byKey(const Key('own')));
    await t.pump();
    EditableText edit(String k) => t.widget<EditableText>(find.descendant(of: find.byKey(Key(k)), matching: find.byType(EditableText)));
    expect(edit('own').keyboardType, TextInputType.none, reason: 'система не должна показывать свою клавиатуру');
    expect(edit('own').autocorrect, isFalse);
    expect(edit('own').enableIMEPersonalizedLearning, isFalse);
    expect(edit('normal').keyboardType, isNot(TextInputType.none), reason: 'проверка самого теста: обычное поле — с системной клавиатурой');
  });
}
