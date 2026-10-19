import 'package:flutter/services.dart';

/// Переслать текст через системное окно «Поделиться» (мессенджер, заметки, кошелёк). Возвращает false, если системное окно не открылось.
Future<bool> shareText(String text) async {
  try {
    return await const MethodChannel('com.yandi.yandi_mobile/files').invokeMethod<bool>('shareText', {'text': text}) ?? false;
  } catch (_) {
    return false;
  }
}
