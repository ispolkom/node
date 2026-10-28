import 'dart:convert';

/// Сигнал звонка между двумя телефонами владельца. Идёт отдельным «живым» кадром, зашифрованным телефоном для телефона
/// (узел его не читает и не хранит). Внутри: тип, номер звонка, и для установки соединения описание сеанса или кандидат.
class CallSignal {
  static const String marker = '\u0001yandi-call:';

  /// invite — позвонить, ringing — получатель получил приглашение, accept — принять, reject — отклонить, busy — занят, hangup — завершить,
  /// offer / answer — описание сеанса WebRTC, ice — сетевой кандидат.
  final String type;
  final String cid;
  final bool   video;
  final int    ts;
  final String? sdp;
  final Map<String, dynamic>? cand;

  const CallSignal({required this.type, required this.cid, this.video = false, required this.ts, this.sdp, this.cand});

  static CallSignal now(String type, String cid, {bool video = false, String? sdp, Map<String, dynamic>? cand}) =>
      CallSignal(type: type, cid: cid, video: video, ts: DateTime.now().millisecondsSinceEpoch, sdp: sdp, cand: cand);

  String encode() => marker + jsonEncode({
    'v': 1, 't': type, 'cid': cid, 'video': video, 'ts': ts,
    if (sdp != null) 'sdp': sdp,
    if (cand != null) 'cand': cand,
  });

  static CallSignal? tryParse(String text) {
    if (!text.startsWith(marker)) return null;
    try {
      final m = jsonDecode(text.substring(marker.length)) as Map<String, dynamic>;
      // та же проверка, что на странице узла (phone-call.js): версия 1, тип — строка, время — целое число, video — bool
      if (m['v'] != 1 || m['t'] is! String || m['ts'] is! int) return null;
      if (m['video'] != null && m['video'] is! bool) return null;
      final cid = m['cid'] as String;
      if (cid.length < 8 || cid.length > 64) return null;
      // кандидат ICE — только с полями нужных типов: иначе обработчик звонка упал бы на приведении типов
      final c = m['cand'];
      if (c != null) {
        if (c is! Map || c['candidate'] is! String || (c['sdpMid'] != null && c['sdpMid'] is! String) || (c['sdpMLineIndex'] != null && c['sdpMLineIndex'] is! int)) {
          return null;
        }
      }
      return CallSignal(
        type:  m['t'] as String,
        cid:   cid,
        video: (m['video'] as bool?) ?? false,
        ts:    (m['ts'] as num).toInt(),
        sdp:   m['sdp'] as String?,
        cand:  (m['cand'] as Map?)?.cast<String, dynamic>(),
      );
    } catch (_) {
      return null;
    }
  }

  /// Не старше полутора минут: устаревший звонок «звонить» не должен.
  bool get fresh => (DateTime.now().millisecondsSinceEpoch - ts).abs() < 90 * 1000;
}
