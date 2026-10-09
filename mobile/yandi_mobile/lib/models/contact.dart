class Contact {
  final String peerId;
  final String displayName;
  bool   online;
  final bool   isManual;

  /// Человек (или устройство) добавлен в контакты: свой контакт узла или сохранённый в приложении.
  /// Остальные (например, другие телефоны владельца) видны в списке, но не в контактах.
  final bool   saved;

  Contact({
    required this.peerId,
    required this.displayName,
    required this.online,
    required this.isManual,
    bool? saved,
  }) : saved = saved ?? isManual;

  Contact copyWith({bool? online, String? displayName, bool? saved}) => Contact(
    peerId:      peerId,
    displayName: displayName ?? this.displayName,
    online:      online      ?? this.online,
    isManual:    isManual,
    saved:       saved       ?? this.saved,
  );

  @override
  bool operator ==(Object other) =>
      other is Contact && other.peerId == peerId;

  @override
  int get hashCode => peerId.hashCode;
}
