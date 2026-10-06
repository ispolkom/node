class Contact {
  final String peerId;
  final String displayName;
  bool   online;
  final bool   isManual;

  Contact({
    required this.peerId,
    required this.displayName,
    required this.online,
    required this.isManual,
  });

  Contact copyWith({bool? online, String? displayName}) => Contact(
    peerId:      peerId,
    displayName: displayName ?? this.displayName,
    online:      online      ?? this.online,
    isManual:    isManual,
  );

  @override
  bool operator ==(Object other) =>
      other is Contact && other.peerId == peerId;

  @override
  int get hashCode => peerId.hashCode;
}
