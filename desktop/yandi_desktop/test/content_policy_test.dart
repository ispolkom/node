import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/content_policy.dart';

void main() {
  test('ссылки находятся во всех обычных видах', () {
    for (final t in [
      'смотри https://example.com/a?b=1', 'http://x.y', 'зайди на www.example.org', 'пиши на example.com', 'ЗАЙДИ example.RU тут',
      'сайт сайт.рф', 'ftp://host/file', 'mailto:a@b.com', 'скачай установка.exe', 'a.b.c.example.net/путь', 't.me', 'bit.ly/abc',
    ]) {
      expect(findLink(t), isNotNull, reason: t);
    }
  });

  test('обычный текст ссылкой не считается', () {
    for (final t in [
      'т.е. вот так', 'версия v1.2.3 готова', 'Привет.Как дела', '1.5 литра', 'и.т.д.', 'Mr.Smith пришёл', 'отчёт.pdf во вложении',
      'фото.png и видео.mp4', 'лист.docx', 'ёлка.да', 'число 3.14', 'e.g. пример', 'приглашение YANDI-GROUP-1:abc.def',
    ]) {
      expect(findLink(t), isNull, reason: '$t -> ${findLink(t)}');
    }
  });

  test('замена и удаление ссылок', () {
    expect(stripLinks('раз https://a.b/c два www.x.ru три'), 'раз [ссылка удалена] два [ссылка удалена] три');
    expect(removeLinks('раз https://a.b/c два'), 'раз два');
    expect(stripLinks('без ссылок'), 'без ссылок');
  });

  test('имена вложений по белому списку', () {
    for (final ok in ['фото.PNG', 'отчёт.pdf', 'голос.m4a', 'клип.MP4', 'таблица.xlsx']) {
      expect(attachmentProblem(ok), isNull, reason: ok);
    }
    for (final bad in ['вирус.exe', 'скрипт.sh', 'архив.zip', 'макрос.docm', 'старый.doc', 'страница.html', 'x.tgfr', 'без_расширения', 'картинка.svg', 'отчёт.pdf.exe', 'отчёт.exe.pdf']) {
      expect(attachmentProblem(bad), isNotNull, reason: bad);
    }
  });

  test('содержимое соответствует расширению, исполняемое отклоняется', () {
    expect(contentProblem('a.png', [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]), isNull);
    expect(contentProblem('a.pdf', '%PDF-1.7'.codeUnits), isNull);
    expect(contentProblem('a.png', 'MZ\x90'.codeUnits), isNotNull);
    expect(contentProblem('a.pdf', '#!/bin/sh\n'.codeUnits), isNotNull);
    expect(contentProblem('a.png', '%PDF-1.7'.codeUnits), isNotNull);
    expect(contentProblem('v.amr', '#!AMR\n'.codeUnits), isNull);
  });

  test('архив под видом документа не проходит, настоящий контейнер проходит', () {
    List<int> zip(String first) {
      final n = first.length;
      return [0x50, 0x4b, 0x03, 0x04, ...List.filled(22, 0), n & 0xff, n >> 8, 0, 0, ...first.codeUnits, ...List.filled(20, 0)];
    }
    expect(contentProblem('a.docx', zip('[Content_Types].xml')), isNull);
    expect(contentProblem('a.docx', zip('setup.exe')), isNotNull);
    expect(contentProblem('a.odt', zip('mimetype')), isNull);
    expect(contentProblem('a.odt', zip('run.js')), isNotNull);
  });

  test('разделители IDN и punycode-зона — ссылка', () {
    for (final t in ['example。com', 'example．com', 'example｡com', 'сайт.xn--p1ai', 'пример.xn--p1ai/путь']) {
      expect(findLink(t), isNotNull, reason: t);
    }
    expect(stripLinks('зайди на site.xn--p1ai тут').contains('xn--'), isFalse, reason: 'зона вычищается целиком');
    expect(findLink('мы встретились. потом разошлись'), isNull);
  });
}
