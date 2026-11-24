import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/screens/call_screen.dart';

void main() {
  test('на экране звонка у имени нет значка устройства: важно кто звонит, а не с чего', () {
    expect(displayCallName('💻 Компьютер'), 'Компьютер');
    expect(displayCallName('📱 Мама'), 'Мама');
    expect(displayCallName('  📱️ YANDI Mobile (767b)'), 'YANDI Mobile (767b)');
    expect(displayCallName('Бабушка'), 'Бабушка');
    expect(displayCallName(''), '');
  });
}
