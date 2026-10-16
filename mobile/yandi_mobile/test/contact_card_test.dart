import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/contact_card.dart';

void main() {
  test('карточка контакта: туда-обратно, имя чистится', () {
    final t = ContactCardMsg.encode('d' * 16, 'd' * 64, 'Кор\u0007\nX');
    final c = ContactCardMsg.tryParse(t)!;
    expect(c.shortId, 'd' * 16);
    expect(c.fullId, 'd' * 64);
    expect(c.name.contains('\n'), isFalse);
    expect(c.name.startsWith('Кор'), isTrue);
  });
  test('мусор и подделка отвергаются', () {
    expect(ContactCardMsg.tryParse('обычный текст'), isNull);
    expect(ContactCardMsg.tryParse('\u0001yandi-ct:не json'), isNull);
    expect(ContactCardMsg.tryParse('\u0001yandi-ct:{"s":"zz"}'), isNull);
    // полный адрес, не совпадающий с коротким, отбрасывается: сохранить по нему нельзя
    final c = ContactCardMsg.tryParse('\u0001yandi-ct:{"s":"${'d' * 16}","i":"${'e' * 64}","n":"x"}')!;
    expect(c.fullId, '');
  });
}
