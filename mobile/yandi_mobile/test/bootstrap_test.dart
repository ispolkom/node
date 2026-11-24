import 'dart:convert';
import 'dart:io';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/bootstrap_list.dart';
import 'package:yandi_mobile/models/guest_node.dart';
import 'package:yandi_mobile/services/bootstrap_loader.dart';
import 'package:yandi_mobile/services/guest_pair.dart';
import 'package:yandi_mobile/services/node_hello.dart';

const _testSigner = '57a7662d45d2cbcbc855355f2154325fd6488cc834e802c5c8a16ff07352a5a5';
String get _guestList => File('test/fixtures/bootstrap_guest.json').readAsStringSync();
String get _repoList => File('../../bootstrap/bootstrap.json').readAsStringSync();

void main() {
  test('закреплённый ключ приложения совпадает с bootstrap/signer.pub, вшитая копия списка — с файлом в репозитории', () {
    expect(bootstrapSigners.single, File('../../bootstrap/signer.pub').readAsStringSync().trim().toLowerCase());
    expect(File('assets/bootstrap.json').readAsStringSync(), _repoList, reason: 'вшитая копия обновляется вместе с bootstrap/bootstrap.json');
  });

  test('список, подписанный узловым инструментом (Rust), проверяется приложением: подпись, срок, откат, чужой ключ', () async {
    final d = BootstrapDoc.parse(_repoList)!;
    final now = d.issued + 1000;
    expect(await d.verify(nowSecs: now), isNull, reason: 'настоящий подписанный список принимается');
    expect(await d.verify(nowSecs: d.expires + 1), BootstrapProblem.expired);
    expect(await d.verify(nowSecs: now, knownSequence: d.sequence + 1), BootstrapProblem.rollback);
    expect(await d.verify(nowSecs: now, knownSequence: d.sequence), isNull);
    expect(await d.verify(nowSecs: now, signers: [_testSigner]), BootstrapProblem.unknownSigner);
    // подмена адреса или роли в подписанном списке ломает подпись
    final tampered = jsonDecode(_repoList) as Map<String, dynamic>;
    (tampered['entries'][0]['addr'] as List)[0] = '203.0.113.99:9000';
    expect(await BootstrapDoc.parse(jsonEncode(tampered))!.verify(nowSecs: now), BootstrapProblem.badSignature);
    final t2 = jsonDecode(_repoList) as Map<String, dynamic>;
    t2['sequence'] = 99;
    expect(await BootstrapDoc.parse(jsonEncode(t2))!.verify(nowSecs: now), BootstrapProblem.badSignature);
  });

  test('запись с адресом для телефона и ролью guest (поле mobile, region = null): подпись Rust сходится, узлы для гостей отбираются', () async {
    final d = BootstrapDoc.parse(_guestList)!;
    expect(await d.verify(nowSecs: 1792500000, signers: [_testSigner]), isNull, reason: 'канонический JSON совпадает с узловым, включая mobile и null');
    final g = d.guestNodes();
    expect(g.map((n) => '${n.host}:${n.port}').toList(), ['203.0.113.9:8444', '2001:db8::9:8444']);
    expect(g.first.name, 'NL · 203.0.113.9');
    expect(g.first.key, '0f' * 32);
    expect(g.first.fingerprint, '', reason: 'отпечаток не хранится: его даёт представление узла');
  });

  test('разбор отвергает мусор: чужие роли, guest без адреса для телефона, плохие адреса, обрезка', () {
    Map<String, dynamic> base() => jsonDecode(_guestList) as Map<String, dynamic>;
    expect(BootstrapDoc.parse('не json'), isNull);
    var m = base();
    m['entries'][0]['roles'] = ['entry', 'boss'];
    expect(BootstrapDoc.parse(jsonEncode(m)), isNull);
    m = base();
    m['entries'][0]['mobile'] = [];
    expect(BootstrapDoc.parse(jsonEncode(m)), isNull, reason: 'guest без адреса для телефона');
    m = base();
    m['entries'][0]['mobile'] = ['bad host:8444'];
    expect(BootstrapDoc.parse(jsonEncode(m)), isNull);
    m = base();
    m['entries'][0]['id'] = 'zz';
    expect(BootstrapDoc.parse(jsonEncode(m)), isNull);
    expect(validMobileAddr('203.0.113.9:8444'), isTrue);
    expect(validMobileAddr('node.example:443'), isTrue);
    expect(validMobileAddr('[2001:db8::9]:8444'), isTrue);
    expect(validMobileAddr('203.0.113.9:0'), isFalse);
    expect(validMobileAddr('203.0.113.9:70000'), isFalse);
    expect(validMobileAddr('2001:db8::9:8444'), isFalse, reason: 'IPv6 без скобок неоднозначен');
  });

  group('загрузка списка', () {
    BootstrapLoader loader({Map<String, String?> net = const {}, String? cache, String emb = '', List<String>? written}) => BootstrapLoader(
          fetch: (u) async => net[u],
          readCache: () async => cache,
          writeCache: (s) async => written?.add(s),
          embedded: () async => emb,
          nowSecs: () => 1792500000,
          signers: [_testSigner],
        );

    test('сеть главнее: верный список принимается и запоминается; нет сети — копия на телефоне; нет и её — вшитая', () async {
      final written = <String>[];
      var r = await loader(net: {BootstrapLoader.urls[0]: _guestList}, written: written).load();
      expect(r!.source, BootstrapSource.network);
      expect(written, [_guestList]);
      r = await loader(net: {BootstrapLoader.urls[1]: _guestList}).load();
      expect(r!.source, BootstrapSource.network, reason: 'второй источник (сайт), если первый недоступен');
      r = await loader(cache: _guestList).load();
      expect(r!.source, BootstrapSource.cache);
      r = await loader(emb: _guestList).load();
      expect(r!.source, BootstrapSource.embedded);
      expect(await loader().load(), isNull, reason: 'ничего верного нет');
    });

    test('подделка из сети (чужая подпись, мусор) не принимается и не затирает копию; старый выпуск не заменяет новый', () async {
      final forged = jsonDecode(_guestList) as Map<String, dynamic>;
      (forged['entries'][0]['mobile'] as List)[0] = '203.0.113.66:8444';
      final written = <String>[];
      final r = await loader(net: {BootstrapLoader.urls[0]: jsonEncode(forged), BootstrapLoader.urls[1]: 'мусор'}, cache: _guestList, written: written).load();
      expect(r!.source, BootstrapSource.cache, reason: 'подделка отвергнута, взята прежняя проверенная копия');
      expect(written, isEmpty);
      // из сети пришёл выпуск ниже того, что уже есть в копии: откат не принимается
      final older = jsonDecode(_guestList) as Map<String, dynamic>..['sequence'] = 1;
      final r2 = await loader(net: {BootstrapLoader.urls[0]: jsonEncode(older)}, cache: _guestList).load();
      expect(r2!.source, BootstrapSource.cache);
    });
  });

  group('представление узла', () {
    // вектор из теста узла the_hello_message_is_a_fixed_vector…: ключ [7;32], число 01..08, отпечаток «ab»×32
    const pub = 'ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c';
    const sig = 'cc067d4f44c4801f3e08d9c4fe02b7dcbfd3ea1c83b88014a8608b9143a4e0e67187aeab73100971aeafa3a3ad656f7bbcae5e0e7f88c56603e2376f5e039709';
    final fp = 'ab' * 32;
    final nonce = [1, 2, 3, 4, 5, 6, 7, 8];
    final node = GuestNode('N', '203.0.113.9', 8444, '', id: 'c' * 64, key: pub);

    HelloTransportFake t({String? certFp, String? nodeId, String? key, String? signature, String? claimed}) => HelloTransportFake(HelloReply({
          'node_id': nodeId ?? 'c' * 64,
          'key': key ?? pub,
          'tls_fingerprint': claimed ?? fp,
          'signature': signature ?? sig,
        }, certFp ?? fp));

    test('узел, подписавший свой сертификат своим ключом из списка, принимается; возвращается проверенный отпечаток', () async {
      expect(await verifyNodeHello(node, transport: t(), nonce: nonce), fp);
    });

    test('подмена не проходит: чужой сертификат, чужой номер или ключ, подпись другого числа, мусор', () async {
      Future<void> refused(HelloTransportFake tr, {List<int>? n}) async => expectLater(verifyNodeHello(node, transport: tr, nonce: n ?? nonce), throwsA(isA<GuestPairError>()));
      await refused(t(certFp: 'cd' * 32), n: nonce); // по сети пришёл другой сертификат, чем подписанный
      await refused(t(nodeId: 'd' * 64));
      await refused(t(key: '11' * 32));
      await refused(t(), n: [9, 9, 9, 9, 9, 9, 9, 9]); // повтор чужого ответа на другое число
      await refused(t(signature: '00' * 64));
      await refused(t(signature: 'не hex'));
      await expectLater(verifyNodeHello(node, transport: HelloTransportFake.failing(), nonce: nonce), throwsA(isA<GuestPairError>()));
      // узел без ключа в списке проверять нечем
      final noKey = GuestNode('N', '203.0.113.9', 8444, '', id: 'c' * 64, key: '');
      await expectLater(verifyNodeHello(noKey, transport: t(), nonce: nonce), throwsA(isA<GuestPairError>()));
    });

    test('сообщение для подписи совпадает с узловым: метка, число, отпечаток в нижнем регистре', () {
      final m = helloMessage(nonce, 'AB' * 32);
      expect(utf8.decode(m.sublist(0, 22)), 'YANDI-MOBILE-HELLO-V1\u0000');
      expect(m.sublist(22, 30), nonce);
      expect(utf8.decode(m.sublist(30)), fp);
      // подпись из теста узла сходится именно с таким сообщением
      expect(Ed25519().verify(m, signature: Signature(List<int>.generate(64, (i) => int.parse(sig.substring(i * 2, i * 2 + 2), radix: 16)), publicKey: SimplePublicKey(List<int>.generate(32, (i) => int.parse(pub.substring(i * 2, i * 2 + 2), radix: 16)), type: KeyPairType.ed25519))), completion(isTrue));
    });
  });
}

class HelloTransportFake implements HelloTransport {
  final HelloReply? reply;
  HelloTransportFake(HelloReply this.reply);
  HelloTransportFake.failing() : reply = null;
  @override
  Future<HelloReply> hello(String host, int port, String nonceHex) async {
    if (reply == null) throw const SocketException('нет связи');
    return reply!;
  }
}
