// Destructive fixtures only: sockets belong to two freshly allocated /tmp nodes.
import 'dart:async';
import 'dart:io';
import '../lib/transport/api.dart';
import '../lib/domain/shell_repository.dart';
import '../lib/domain/states.dart';

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
    final contact = (await ra.contacts()).firstWhere(
      (c) => c['name'] == 'Узел Б',
    );
    await ra.rename(contact, 'Узел Б новое имя');
    if (!(await ra.contacts()).any((c) => c['name'] == 'Узел Б новое имя'))
      throw StateError('rename');
    await ra.saveProfile('Профиль оболочки');
    if ((await ra.profile())['display_name'] != 'Профиль оболочки')
      throw StateError('profile');
    await ra.remove(contact['id'] as String);
    if ((await ra.contacts()).any((c) => c['id'] == contact['id']))
      throw StateError('remove');
    stdout.writeln('PASS: desktop M2 Unix API card/contacts/chat A↔B/profile');
  } catch (e) {
    stderr.writeln(e is ApiFailure ? e.message : 'Проверка M2 не пройдена: $e');
    exitCode = 1;
  } finally {
    a.cancel();
    b.cancel();
  }
}
