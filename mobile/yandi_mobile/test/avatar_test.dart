import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/avatar_msg.dart';
import 'package:yandi_mobile/services/avatar_service.dart';

Uint8List fakePng(int seed, {int size = 200}) {
  final b = Uint8List(size);
  b.setRange(0, 8, const [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
  for (var i = 8; i < size; i++) { b[i] = (i * 31 + seed) % 256; }
  return b;
}

void main() {
  late Directory dir;
  late String? stored;
  late AvatarService svc;
  final sealer = AvatarSealer(() async => List<int>.generate(32, (i) => i + 5));
  AvatarService make() => AvatarService(dir: Directory('${dir.path}/avatars'), read: () async => stored, write: (s) async => stored = s, sealer: sealer);
  setUp(() { dir = Directory.systemTemp.createTempSync('yandi_ava'); stored = null; svc = make(); });
  tearDown(() => dir.deleteSync(recursive: true));

  test('сообщения: разбор, отказ на кривом хэше и на слишком большой картинке', () {
    final p = fakePng(1);
    final h = AvatarMsg.hashOf(p);
    expect(AvatarMsg.tryParse(AvatarMsg.announce(h))!.hash, h);
    expect(AvatarMsg.tryParse(AvatarMsg.announce(''))!.hash, '');
    expect(AvatarMsg.tryParse(AvatarMsg.request(h))!.kind, 'q');
    final m = AvatarMsg.tryParse(AvatarMsg.image(h, p))!;
    expect(m.kind, 'a');
    expect(m.png, p);
    expect(AvatarMsg.tryParse('\u0001yandi-avh:{"h":"zz"}'), isNull);
    expect(AvatarMsg.tryParse('\u0001yandi-avq:{"h":""}'), isNull);
    expect(AvatarMsg.tryParse(AvatarMsg.image(h, fakePng(1, size: 80 * 1024))), isNull);
    expect(AvatarMsg.isAvatar(AvatarMsg.announce(h)), isTrue);
    expect(AvatarMsg.isAvatar('обычный текст'), isFalse);
  });

  test('чужая картинка: объявление → запрос → картинка → файл и привязка', () async {
    final p = fakePng(2), h = AvatarMsg.hashOf(p);
    final sent = <String>[];
    await svc.onAnnounce('peerA', h, (t) async => sent.add(t));
    expect(sent, [AvatarMsg.request(h)]);
    expect(svc.bytesForPeer('peerA'), isNull);
    expect(await svc.onImage('peerA', h, p), isTrue);
    expect(svc.hashForPeer('peerA'), h);
    expect(svc.bytesForPeer('peerA'), p);
  });

  test('то же объявление повторно не вызывает нового запроса', () async {
    final p = fakePng(3), h = AvatarMsg.hashOf(p);
    final sent = <String>[];
    await svc.onAnnounce('peerA', h, (t) async => sent.add(t));
    await svc.onImage('peerA', h, p);
    await svc.onAnnounce('peerA', h, (t) async => sent.add(t));
    expect(sent.length, 1);
  });

  test('незапрошенная, подменённая и некорректная картинки отвергаются', () async {
    final p = fakePng(4), h = AvatarMsg.hashOf(p);
    expect(await svc.onImage('peerA', h, p), isFalse, reason: 'не просили');
    await svc.onAnnounce('peerA', h, (t) async {});
    expect(await svc.onImage('peerA', h, fakePng(5)), isFalse, reason: 'хэш не совпал');
    await svc.onAnnounce('peerA', h, (t) async {});
    expect(await svc.onImage('peerA', h, Uint8List.fromList(List.filled(300, 7))), isFalse, reason: 'не PNG');
    await svc.onAnnounce('peerA', h, (t) async {});
    expect(await svc.onImage('peerA', h, p, decodes: (_) async => false), isFalse, reason: 'декодер отказал');
    expect(svc.bytesForPeer('peerA'), isNull);
  });

  test('картинка запрошена у одного контакта, пришла от другого — не принимается', () async {
    final p = fakePng(6), h = AvatarMsg.hashOf(p);
    await svc.onAnnounce('peerA', h, (t) async {});
    expect(await svc.onImage('peerB', h, p), isFalse);
  });

  test('одинаковая картинка у разных людей: один файл, привязки раздельные', () async {
    final p = fakePng(7), h = AvatarMsg.hashOf(p);
    await svc.onAnnounce('peerA', h, (t) async {});
    await svc.onImage('peerA', h, p);
    final sent = <String>[];
    await svc.onAnnounce('peerB', h, (t) async => sent.add(t));
    expect(sent, isEmpty, reason: 'такая картинка уже есть — не качаем');
    expect(svc.hashForPeer('peerB'), h);
    expect(Directory('${dir.path}/avatars').listSync().whereType<File>().length, 1);
    await svc.forgetPeer('peerA');
    expect(svc.bytesForPeer('peerA'), isNull);
    expect(svc.bytesForPeer('peerB'), isNotNull, reason: 'файл нужен второму контакту — остаётся');
    await svc.forgetPeer('peerB');
    expect(Directory('${dir.path}/avatars').listSync().whereType<File>(), isEmpty, reason: 'на файл никто не ссылается — удалён');
  });

  test('пустой хэш = контакт убрал картинку', () async {
    final p = fakePng(8), h = AvatarMsg.hashOf(p);
    await svc.onAnnounce('peerA', h, (t) async {});
    await svc.onImage('peerA', h, p);
    await svc.onAnnounce('peerA', '', (t) async {});
    expect(svc.hashForPeer('peerA'), isNull);
  });

  test('своя картинка: отдаётся только по запросу с её хэшем', () async {
    final p = fakePng(9);
    final h = await svc.setMine(p);
    expect(svc.myBytes, p);
    final sent = <String>[];
    await svc.onRequest('peerA', AvatarMsg.hashOf(fakePng(10)), (t) async => sent.add(t));
    expect(sent, isEmpty, reason: 'запросили не ту');
    await svc.onRequest('peerA', h, (t) async => sent.add(t));
    expect(sent.length, 1);
    expect(AvatarMsg.tryParse(sent.single)!.png, p);
    await svc.clearMine();
    await svc.onRequest('peerA', h, (t) async => sent.add(t));
    expect(sent.length, 1, reason: 'после удаления отдавать нечего');
    expect(svc.myBytes, isNull);
  });

  test('привязки и своя картинка переживают перезапуск', () async {
    final p = fakePng(11), h = AvatarMsg.hashOf(p);
    await svc.onAnnounce('peerA', h, (t) async {});
    await svc.onImage('peerA', h, p);
    final mine = await svc.setMine(fakePng(12));
    final again = make();
    await again.load();
    expect(again.hashForPeer('peerA'), h);
    expect(again.myHash, mine);
    expect(again.bytesForPeer('peerA'), isNotNull);
  });

  test('setMine отвергает не-PNG и слишком большое', () async {
    expect(() => svc.setMine(Uint8List.fromList(List.filled(300, 1))), throwsStateError);
    expect(() => svc.setMine(fakePng(1, size: 60 * 1024)), throwsStateError);
  });

  test('две стороны целиком: А ставит картинку, Б получает её через объявление → запрос → картинку', () async {
    final dirB = Directory('${dir.path}/b');
    String? storedB;
    final a = svc;
    final b = AvatarService(dir: Directory('${dirB.path}/avatars'), read: () async => storedB, write: (s) async => storedB = s, sealer: sealer);
    final png = fakePng(21);
    final h = await a.setMine(png);

    // «провод»: то, что А отправил Б, и наоборот
    final toB = <String>[], toA = <String>[];
    toB.add(AvatarMsg.announce(h));                       // А: моя картинка такая
    var m = AvatarMsg.tryParse(toB.removeLast())!;
    await b.onAnnounce('A', m.hash, (t) async => toA.add(t));   // Б: такой у меня нет, пришли
    m = AvatarMsg.tryParse(toA.removeLast())!;
    expect(m.kind, 'q');
    await a.onRequest('B', m.hash, (t) async => toB.add(t));    // А: вот она
    m = AvatarMsg.tryParse(toB.removeLast())!;
    expect(m.kind, 'a');
    expect(await b.onImage('A', m.hash, m.png!), isTrue);
    expect(b.bytesForPeer('A'), png);
    expect(b.hashForPeer('A'), h);

    // А меняет картинку: Б получает новый хэш, запрашивает, старый файл удаляется как неиспользуемый
    final png2 = fakePng(22);
    final h2 = await a.setMine(png2);
    await b.onAnnounce('A', h2, (t) async => toA.add(t));
    final q = AvatarMsg.tryParse(toA.removeLast())!;
    await a.onRequest('B', q.hash, (t) async => toB.add(t));
    final img = AvatarMsg.tryParse(toB.removeLast())!;
    expect(await b.onImage('A', img.hash, img.png!), isTrue);
    expect(b.hashForPeer('A'), h2);
    expect(Directory('${dirB.path}/avatars').listSync().whereType<File>().length, 1);
  });

  test('на диске картинка зашифрована; старый открытый .png шифруется и стирается при загрузке', () async {
    final p = fakePng(21), h = AvatarMsg.hashOf(p);
    await svc.onAnnounce('peerA', h, (t) async {});
    await svc.onImage('peerA', h, p);
    final enc = File('${dir.path}/avatars/$h.yav');
    expect(enc.existsSync(), isTrue);
    expect(enc.readAsBytesSync().length, p.length + 28, reason: 'nonce 12 + метка 16');
    expect(String.fromCharCodes(enc.readAsBytesSync()).contains(String.fromCharCodes(p.sublist(8, 40))), isFalse);
    // чужой ключ не открывает
    final other = AvatarService(dir: Directory('${dir.path}/avatars'), read: () async => stored, write: (s) async => stored = s,
        sealer: AvatarSealer(() async => List<int>.filled(32, 1)));
    await other.load();
    expect(other.bytesForPeer('peerA'), isNull);

    // старый открытый файл
    final q = fakePng(22), hq = AvatarMsg.hashOf(q);
    File('${dir.path}/avatars/$hq.png').writeAsBytesSync(q);
    stored = '{"me":null,"peers":{"peerB":"$hq"}}';
    final mig = make();
    await mig.load();
    expect(mig.bytesForPeer('peerB'), q);
    expect(File('${dir.path}/avatars/$hq.png').existsSync(), isFalse, reason: 'открытый оригинал стёрт');
    expect(File('${dir.path}/avatars/$hq.yav').existsSync(), isTrue);
  });
}
