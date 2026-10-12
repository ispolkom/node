// Only run against disposable chaos fixtures.
import '../lib/transport/api.dart';
import '../lib/domain/shell_repository.dart';
Future<void> main(List<String> args) async {
  final api = UnixApi(args[0]);
  try {
    for (final path in ['/api/mobile/devices', '/api/mobile/other-devices']) {
      ShellRepository.list(await api.request(path), 'devices');
    }
    try {
      await api.request('/api/mobile/phone-action', body: {'peer': 'd' * 64, 'action': 'invalid'});
      throw StateError('invalid phone action accepted');
    } on ApiFailure catch (e) { if (e.status != 400) rethrow; }
    print('PASS: M5 device list and invalid-action rejection via Unix');
  } finally { api.cancel(); }
}
