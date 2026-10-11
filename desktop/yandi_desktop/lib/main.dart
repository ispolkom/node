import 'dart:io';
import 'package:flutter/material.dart';
import 'app.dart';
import 'transport/api.dart';

void main(List<String> args) {
  WidgetsFlutterBinding.ensureInitialized();
  try {
    runApp(DesktopApp(api: UnixApi(resolveSocket(args, Platform.environment))));
  } on FormatException catch (e) {
    runApp(
      MaterialApp(
        home: Scaffold(
          body: Center(child: Text('Ошибка запуска: ${e.message}')),
        ),
      ),
    );
  }
}
