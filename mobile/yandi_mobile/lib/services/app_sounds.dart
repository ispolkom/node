import 'package:audioplayers/audioplayers.dart';

/// The same five bundled tones are used by web, desktop and mobile.
abstract final class AppSounds {
  static const assets = <String, String>{
    'messages': 'icq.mp3',
    'groups': 'icq-online.mp3',
    'calls': 'calling.mp3',
    'videoCalls': 'sqype.mp3',
    'dialing': 'call_out.mp3',
  };
  static const labels = <String, String>{
    'messages': 'Личные сообщения',
    'groups': 'События в группах',
    'calls': 'Аудиозвонки',
    'videoCalls': 'Видеозвонки',
    'dialing': 'Гудки при дозвоне',
  };

  static AudioContext context(String kind) => AudioContext(
    android: AudioContextAndroid(
      contentType: AndroidContentType.sonification,
      usageType: kind == 'calls' || kind == 'videoCalls'
          ? AndroidUsageType.notificationRingtone
          : kind == 'dialing'
              ? AndroidUsageType.voiceCommunicationSignalling
              : AndroidUsageType.notification,
      audioFocus: kind == 'messages' || kind == 'groups'
          ? AndroidAudioFocus.gainTransientMayDuck
          : AndroidAudioFocus.gainTransient,
    ),
    iOS: AudioContextIOS(category: AVAudioSessionCategory.ambient),
  );
}
