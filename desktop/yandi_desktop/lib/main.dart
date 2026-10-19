import 'dart:io';
import 'package:flutter/material.dart';
import 'app.dart';
import 'theme.dart';
import 'transport/api.dart';

import 'embedded_player.dart';
import 'voice.dart' show sweepTempFiles;

void main(List<String> args) {
  WidgetsFlutterBinding.ensureInitialized();
  try {
    sweepTempFiles();
    EmbeddedPlayer.init(); // встроенный проигрыватель, если на машине есть libmpv
    runApp(DesktopApp(api: UnixApi(resolveSocket(args, Platform.environment))));
  } on FormatException catch (e) {
    runApp(
      MaterialApp(
        theme: Icq.theme,
        home: Scaffold(
          body: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 500),
              child: NoticePanel('Ошибка запуска: ${e.message}', error: true),
            ),
          ),
        ),
      ),
    );
  }
}
