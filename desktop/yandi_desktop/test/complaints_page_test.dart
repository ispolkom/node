import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/complaints_page.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/transport/api.dart';

class _Api implements Api {
  final calls = <String>[];
  final bodies = <Map<String, dynamic>?>[];
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) async {
    calls.add(path);
    bodies.add(body);
    if (path == '/api/complaints') {
      return {
        'complaints': [
          {'id': 'c1', 'ts': 1700000000000, 'reporter': 'a' * 64, 'target_kind': 'user', 'target_id': 'b' * 64, 'target_name': 'Боб', 'reason': 'spam', 'status': 'new', 'messages': 0},
        ],
      };
    }
    if (path == '/api/complaints/c1') {
      return {
        'id': 'c1', 'ts': 1700000000000, 'reporter': 'a' * 64, 'target_kind': 'user', 'target_id': 'b' * 64, 'target_name': 'Боб',
        'reason': 'spam', 'text': 'спамит в личку', 'evidence': ['реклама раз'], 'status': 'new', 'thread': [], 'measures': [],
      };
    }
    return {'ok': true};
  }

  @override
  void cancel() {}
}

void main() {
  testWidgets('жалоба видна слева, открывается справа, ответ и предупреждение уходят на узел', (t) async {
    final api = _Api();
    await t.binding.setSurfaceSize(const Size(1400, 900));
    await t.pumpWidget(
      PreferencesScope(
        preferences: AppPreferences(),
        child: MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(child: ComplaintsFeature(api: api, isActive: () => true, onFailure: (_) {})),
          ),
        ),
      ),
    );
    await t.pump();
    await t.pump();
    expect(find.textContaining('Боб'), findsWidgets);
    await t.tap(find.textContaining('Пользователь: Боб').first);
    await t.pump();
    await t.pump();
    expect(find.text('спамит в личку'), findsOneWidget);
    expect(find.textContaining('реклама раз'), findsOneWidget);
    await t.tap(find.text('Предупреждение'));
    await t.pump();
    expect(api.calls.contains('/api/complaints/c1/measure'), true);
    final measure = api.bodies[api.calls.lastIndexOf('/api/complaints/c1/measure')];
    expect(measure?['kind'], 'warn');
    await t.pumpWidget(const SizedBox());
    await t.binding.setSurfaceSize(null);
  });
}
