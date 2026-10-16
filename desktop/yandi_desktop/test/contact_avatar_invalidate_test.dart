import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/contact_avatar.dart';

void main() {
  test('смена картинки контакта уведомляет показанные картинки: перечитать, а не ждать пересоздания виджета', () {
    var seen = 0;
    void listener() => seen++;
    ContactAvatar.changes.addListener(listener);
    final before = ContactAvatar.changes.value;
    ContactAvatar.invalidate('ab' * 8);
    expect(ContactAvatar.changes.value, before + 1);
    expect(seen, 1);
    ContactAvatar.changes.removeListener(listener);
  });
}
