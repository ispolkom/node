import 'dart:io';
import 'package:flutter/material.dart';
import 'app.dart';
import 'theme.dart';
import 'transport/api.dart';

void main(List<String> args) {
  WidgetsFlutterBinding.ensureInitialized();
  try {
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
