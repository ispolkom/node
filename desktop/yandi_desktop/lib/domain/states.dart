enum AuthStage { setup, recovery, login }

AuthStage parseAuth(Map<String, dynamic> json) {
  if (json['setup_required'] is! bool || json['recovery_required'] is! bool) {
    throw const FormatException('auth state');
  }
  if (json['setup_required'] == true) return AuthStage.setup;
  if (json['recovery_required'] == true) return AuthStage.recovery;
  return AuthStage.login;
}

class VaultState {
  final bool configured, unlocked, migrationPending;
  final int autoLockSecs;
  final bool hasError;
  VaultState.fromJson(Map<String, dynamic> json)
    : configured = json['configured'] as bool,
      unlocked = json['unlocked'] as bool,
      migrationPending = json['migration_pending'] == true,
      autoLockSecs = (json['auto_lock_secs'] as num).toInt(),
      hasError = json['error'] != null && json['error'] != '';
  String get label => hasError
      ? 'Не удалось открыть закрытую среду'
      : !configured
      ? 'Не создана: закрытые данные сейчас защищены только ключом этой машины.'
      : unlocked
      ? 'Открыта. Узел может читать историю и группы ПК.'
      : 'Закрытая среда заперта: история и группы ПК не читаются, пока не введён пароль.';
}
