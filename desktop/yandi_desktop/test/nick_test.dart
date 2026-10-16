import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/nick.dart';

void main() {
  test('ник считается заданным, только если он не пуст и не равен короткому номеру узла', () {
    expect(nickOf(null), isNull);
    expect(nickOf({'display_name': 'abc123', 'short_id': 'abc123'}), isNull, reason: 'по умолчанию в профиле лежит номер узла');
    expect(nickOf({'display_name': '  ', 'short_id': 'abc123'}), isNull);
    expect(nickOf({'display_name': 'Вера', 'short_id': 'abc123'}), 'Вера');
  });
}
