import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/member_badge.dart';

void main() {
  test('запирание стирает визитки участников из памяти: байты обнулены, кеш пуст', () {
    final png = Uint8List.fromList([1, 2, 3, 4]);
    MemberBadge.cacheForTest('g|a', png);
    expect(MemberBadge.cachedCount, 1);
    MemberBadge.forget();
    expect(MemberBadge.cachedCount, 0);
    expect(png, [0, 0, 0, 0], reason: 'сами байты затёрты, а не только убрана ссылка');
  });
}
