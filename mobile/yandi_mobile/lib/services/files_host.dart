import '../models/contact.dart';

/// Всё, что вкладке «Файлы» нужно от приложения: кому можно переслать, сколько передач идёт (чтобы перечитать список), и сама отправка.
/// Отдельный интерфейс — чтобы вкладку можно было проверять без платформенных плагинов.
abstract class FilesHost {
  List<Contact> get contacts;
  int get transferCount;
  Future<String?> sendFile(String peerId, String path, {String? name});
}
