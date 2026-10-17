import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/password_strength.dart';

void main() {
  test('короткие, словарные и однообразные пароли слабые, фраза из слов хорошая', () {
    for (final weak in ['123456', 'password', 'qwerty', '1234567', 'aaaaaaaa', 'abcdefgh', '12345678', '11111111', 'Пароль']) {
      expect(PasswordStrength.level(weak), PasswordLevel.weak, reason: weak);
    }
    expect(PasswordStrength.level(''), PasswordLevel.empty);
    expect(PasswordStrength.level('correct horse battery staple'), PasswordLevel.good);
    expect(PasswordStrength.level('Tr0ub4dor&3xK!'), isNot(PasswordLevel.weak));
    expect(PasswordStrength.level('моя кошка любит рыбу по вечерам'), PasswordLevel.good);
    // три слова — это подбор слов, а не знаков: «a b c» длинной не считается
    expect(PasswordStrength.level('cat dog sun'), isNot(PasswordLevel.good));
  });
}

