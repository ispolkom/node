import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/screens/vault_screens.dart';

void main() {
  test('экстренное стирание подтверждается только словом (регистр и пробелы по краям не важны)', () {
    expect(VaultPanicScreen.confirmed('СТЕРЕТЬ'), true);
    expect(VaultPanicScreen.confirmed('  стереть '), true);
    for (final ok in ['DELETE', 'delete', ' Delete ']) {
      expect(VaultPanicScreen.confirmed(ok), true, reason: ok);
    }
    for (final no in ['', 'нет', 'стереть всё', 'СТЕРЕТ', 'DELET', 'ERASE', 'СТЕРЕТЬ!']) {
      expect(VaultPanicScreen.confirmed(no), false, reason: no);
    }
  });
}
