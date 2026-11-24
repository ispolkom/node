// Destructive fixtures only: sockets belong to two freshly allocated /tmp nodes.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import '../lib/domain/file_repository.dart';
import '../lib/transport/api.dart';
import '../lib/domain/shell_repository.dart';
import '../lib/domain/states.dart';
import '../lib/domain/group_repository.dart';

Future<void> waitFor(Future<bool> Function() check, String label) async {
  final deadline = DateTime.now().add(const Duration(seconds: 120));
  while (DateTime.now().isBefore(deadline)) {
    try {
      if (await check()) return;
    } on ApiFailure {
      /* safe readiness GET */
    }
    await Future<void>.delayed(const Duration(milliseconds: 500));
  }
  throw StateError(label);
}

Future<void> main(List<String> args) async {
  final a = UnixApi(args[0]), b = UnixApi(args[1]);
  final ra = ShellRepository(a), rb = ShellRepository(b);
  try {
    if (parseAuth(await b.request('/api/auth/state')) == AuthStage.setup) {
      await b.request(
        '/api/auth/setup',
        body: {
          'login_password': 'testlogin123',
          'login_password_repeat': 'testlogin123',
          'master_password': 'test master password 123',
          'master_password_repeat': 'test master password 123',
        },
      );
    }
    await waitFor(
      () async =>
          parseAuth(await b.request('/api/auth/state')) == AuthStage.login,
      'second node startup',
    );
    await waitFor(
      () async =>
          (await ra.trusted())['card'] is String &&
          (await rb.trusted())['card'] is String,
      'cards ready',
    );
    final cardA = (await ra.trusted())['card'] as String;
    final cardB = (await rb.trusted())['card'] as String;
    await ra.addCard(cardB, 'Узел Б');
    await rb.addCard(cardA, 'Узел А');
    final idA = (await a.request('/api/status'))['node_id'] as String;
    final idB = (await b.request('/api/status'))['node_id'] as String;
    await waitFor(() async {
      final peersA = ShellRepository.list(await ra.trusted(), 'peers');
      final peersB = ShellRepository.list(await rb.trusted(), 'peers');
      return peersA.any((p) => p['id'] == idB && p['online'] == true) &&
          peersB.any((p) => p['id'] == idA && p['online'] == true);
    }, 'mutual transport connection');
    final marker = 'desktop-m2-${DateTime.now().microsecondsSinceEpoch}';
    await ra.send(idB, '$marker A→B'); // Never retry POST.
    await waitFor(
      () async =>
          (await rb.history(idA)).any((m) => m['text'] == '$marker A→B'),
      'A→B delivery',
    );
    await rb.send(idA, '$marker B→A');
    await waitFor(
      () async =>
          (await ra.history(idB)).any((m) => m['text'] == '$marker B→A'),
      'B→A delivery',
    );
    final own = (await ra.history(
      idB,
    )).firstWhere((m) => m['text'] == '$marker A→B');
    await ra.edit(idB, own['msg_id'] as String, '$marker edited');
    if (!(await ra.history(
      idB,
    )).any((m) => m['text'] == '$marker edited' && m['edited'] == true))
      throw StateError('local edit');
    await ra.deleteMessage(idB, own['msg_id'] as String);
    if ((await ra.history(idB)).any((m) => m['msg_id'] == own['msg_id']))
      throw StateError('local delete');
    if (!(await rb.history(idA)).any((m) => m['text'] == '$marker A→B'))
      throw StateError('local deletion changed remote history');
    final temporary = await Directory.systemTemp.createTemp('m4-selected-');
    try {
      final content = Uint8List.fromList(List.generate(1501, (i) => i % 251));
      final selected = await File(
        '${temporary.path}/проверка файла.bin',
      ).writeAsBytes(content);
      final filesA = FileRepository(a), filesB = FileRepository(b);
      final attachment = await filesA.upload(
        idB,
        selected,
        phone: false,
        isActive: () => true,
        progress: (_, _) {},
      );
      await waitFor(
        () async => (await rb.history(idA)).any(
          (m) =>
              m['attachment'] is Map &&
              (m['attachment'] as Map)['file_ref'] is Map &&
              ((m['attachment'] as Map)['file_ref'] as Map)['file_id'] ==
                  (attachment['file_ref'] as Map)['file_id'],
        ),
        'file message A→B',
      );
      await waitFor(
        () async =>
            (await b.request(filesB.path(attachment, 'status')))['exists'] ==
            true,
        'file content A→B',
      );
      final received = await b.bytes(filesB.path(attachment, 'content'));
      if (received.length != content.length ||
          List.generate(
            content.length,
            (i) => received[i] == content[i],
          ).contains(false))
        throw StateError('file bytes A→B');
    } finally {
      await temporary.delete(recursive: true);
    }
    await ra.clearHistory(idB);
    if ((await ra.history(idB)).isNotEmpty) throw StateError('clear history');
    // Groups are hosted by one daemon; the two-node scenario above covers
    // transport chat. Group lifecycle uses the same desktop repository on A.
    final groups = GroupRepository(a);
    final gid = await groups.create(
      'Группа оболочки',
      'Технологии',
      'M2',
      'invite',
    );
    await waitFor(
      () async => (await groups.feed(gid))['has_key'] == true,
      'group key',
    );
    await groups.send(gid, '$marker группа');
    await waitFor(
      () async => ((await groups.feed(gid))['messages'] as List).any(
        (m) => m['text'] == '$marker группа',
      ),
      'group text',
    );
    await a.request('/api/groups/devices');
    final groupTemp = await Directory.systemTemp.createTemp('m5-group-');
    try {
      final groupFile = File('${groupTemp.path}/группа файл.bin');
      final groupBytes = Uint8List.fromList(
        List.generate(1537, (i) => i % 251),
      );
      await groupFile.writeAsBytes(groupBytes);
      final uploadedGroup = await a.uploadGroup(
        '/api/groups/$gid/upload?name=${Uri.encodeComponent('группа файл.bin')}',
        groupFile,
        isActive: () => true,
        progress: (_, _) {},
      );
      final downloadedGroup = await a.bytes(
        '/api/groups/$gid/file/${uploadedGroup['seq']}',
      );
      if (base64Encode(downloadedGroup) != base64Encode(groupBytes))
        throw StateError('group file bytes');
      final safeFeed = (await groups.feed(gid)).toString();
      if (safeFeed.contains("'k':") || !safeFeed.contains('группа файл.bin'))
        throw StateError('group file display');
    } finally {
      await groupTemp.delete(recursive: true);
    }
    final info = await groups.info(gid);
    if ((info['group'] as Map)['role'] != 'owner' ||
        (info['members'] as List).isEmpty)
      throw StateError('group owner/members');
    final invite = await groups.act(gid, 'invite-code');
    if (!(invite['invite'] as String).startsWith('YANDI-GROUP-1:$gid:'))
      throw StateError('group invite');
    await groups.act(gid, 'settings', {
      'name': 'Новое имя группы',
      'topic': 'Работа',
      'about': 'Обновлено',
    });
    if (((await groups.info(gid))['group'] as Map)['name'] !=
        'Новое имя группы')
      throw StateError('group settings');
    await groups.send(gid, 'https://example.com/desktop-test');
    await waitFor(
      () async => ((await groups.feed(gid))['messages'] as List).any(
        (m) => m['kind'] == 6,
      ),
      'group link',
    );
    await groups.act(gid, 'rules', {
      'deny': [6],
    });
    if (((await groups.feed(gid))['messages'] as List).any(
      (m) => m['kind'] == 6,
    ))
      throw StateError('group denied snapshot');
    final textRow = ((await groups.feed(gid))['messages'] as List).firstWhere(
      (m) => m['text'] == '$marker группа',
    );
    await groups.act(gid, 'mod', {
      'action': 'delete_entry',
      'seq': textRow['seq'],
    });
    if (((await groups.feed(gid))['messages'] as List).any(
      (m) => m['text'] == '$marker группа',
    ))
      throw StateError('group deleted snapshot');
    await a.request(
      '/api/mobile/groups/$gid/delete',
      body: {'password': 'testlogin123'},
    );
    if (((await groups.groups())['groups'] as List).any((g) => g['id'] == gid))
      throw StateError('group StepUp delete');
    final contact = (await ra.contacts()).firstWhere(
      (c) => c['name'] == 'Узел Б',
    );
    await ra.rename(contact, 'Узел Б новое имя');
    if (!(await ra.contacts()).any((c) => c['name'] == 'Узел Б новое имя'))
      throw StateError('rename');
    final avatar = base64Decode(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==',
    );
    await a.multipart(
      '/api/profile/avatar',
      {},
      'avatar',
      'avatar.png',
      avatar,
    );
    final avatarPath = (await ra.profile())['avatar'] as String;
    if ((await a.bytes(avatarPath)).length != avatar.length)
      throw StateError('avatar upload/view');
    await a.request('/api/profile/me', method: 'PUT', body: {'avatar': ''});
    if ((await ra.profile())['avatar'] != '')
      throw StateError('avatar removal');
    final peerProfile = await a.request('/api/profile/${idB.substring(0, 16)}');
    if (peerProfile['profile'] is! Map) throw StateError('peer profile');
    await ra.saveProfile('Профиль оболочки');
    if ((await ra.profile())['display_name'] != 'Профиль оболочки')
      throw StateError('profile');
    final exported = jsonDecode(
      utf8.decode(await a.bytes('/api/contacts/export')),
    );
    if (exported is! Map || exported['contacts'] is! List)
      throw StateError('contacts export');
    await a.request('/api/peers/trusted/remove', body: {'id': idB});
    if (ShellRepository.list(
          await ra.trusted(),
          'peers',
        ).any((p) => p['id'] == idB) ||
        !(await ra.contacts()).any((c) => c['id'] == contact['id']))
      throw StateError('separate trust revocation');
    await ra.remove(contact['id'] as String);
    if ((await ra.contacts()).any((c) => c['id'] == contact['id']))
      throw StateError('remove');
    stdout.writeln(
      'PASS: desktop M4 Unix API chat edit/delete/clear/file A→B/avatar/profile/export/trust + M2 groups',
    );
  } catch (e) {
    stderr.writeln(e is ApiFailure ? e.message : 'Проверка M2 не пройдена: $e');
    exitCode = 1;
  } finally {
    a.cancel();
    b.cancel();
  }
}
