import 'dart:async';
import 'package:flutter/material.dart';
import 'package:yandi_desktop/app.dart';
import 'design_fixture.dart';

/// Linux-only visual QA entrypoint; production main.dart always uses UnixApi.
void main(List<String> args) {
  WidgetsFlutterBinding.ensureInitialized();
  final scene = args.isEmpty ? 'Главная' : args.first;
  final key = GlobalKey();
  runApp(DesktopApp(key: key, api: DesignApi(scene)));
  if (['offline', 'login', 'setup'].contains(scene)) return;
  Timer(const Duration(milliseconds: 800), () async {
    final dynamic state = key.currentState;
    await state.enter();
    state.setState(() {
      state.page = scene == 'Главная'
          ? 'Панель'
          : scene == 'unlock'
          ? 'Закрытая среда'
          : scene;
      state.formVersion++;
    });
  });
}
