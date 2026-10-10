class Contact {
  final String peerId;
  final String displayName;
  bool   online;
  final bool   isManual;

  /// Человек (или устройство) добавлен в контакты: свой контакт узла или сохранённый в приложении.
  /// Остальные (например, другие телефоны владельца) видны в списке, но не в контактах.
  final bool   saved;

  /// Когда устройство (не контакт) в последний раз было на связи или сопрягалось, мс; 0 — узел не сообщил.
  final int    lastActivityMs;

  Contact({
    required this.peerId,
    required this.displayName,
    required this.online,
    required this.isManual,
    bool? saved,
    this.lastActivityMs = 0,
  }) : saved = saved ?? isManual;

  /// Устройство, не бывшее в сети больше суток и не добавленное в контакты, с главного экрана уходит в «скрытые»
  /// и возвращается само, как только появится в сети (online) — через неделю или через полгода.
  static const Duration hideAfter = Duration(hours: 24);
  bool isStale(int nowMs) => !saved && !online && lastActivityMs > 0 && nowMs - lastActivityMs > hideAfter.inMilliseconds;

  Contact copyWith({bool? online, String? displayName, bool? saved}) => Contact(
    peerId:      peerId,
    displayName: displayName ?? this.displayName,
    online:      online      ?? this.online,
    isManual:    isManual,
    saved:       saved       ?? this.saved,
    lastActivityMs: lastActivityMs,
  );

  @override
  bool operator ==(Object other) =>
      other is Contact && other.peerId == peerId;

  @override
  int get hashCode => peerId.hashCode;
}
