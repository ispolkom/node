import 'dart:async';
import 'transport/api.dart';
import 'tombstone.dart';
import 'preferences.dart';
import 'domain/states.dart';
import 'domain/shell_repository.dart';
import 'domain/group_repository.dart';

/// Bounded session observations, not daemon read receipts. Never polls phones.
class BackgroundAlerts {
  final Api api;
  final AppPreferences preferences;
  final bool Function() allowed;
  final Map<String, ({Set<String> ids, num latest})> baseline = {};
  final Set<String> unread = {};
  int generation = 0;
  bool busy = false, disposed = false;
  String? openPeer;
  Timer? timer;
  BackgroundAlerts(this.api, this.preferences, this.allowed) {
    timer = Timer.periodic(const Duration(seconds: 5), (_) => poll());
  }
  void reset() {
    generation++;
    baseline.clear();
    unread.clear();
    openPeer = null;
  }

  void dispose() {
    disposed = true;
    timer?.cancel();
    reset();
  }

  void opened(String peer) {
    openPeer = peer;
    unread.remove(peer);
  }

  Future<void> poll() async {
    if (disposed || busy || !allowed() || !preferences.background) return;
    busy = true;
    final rev = generation;
    bool active() =>
        !disposed && generation == rev && allowed() && preferences.background;
    try {
      final auth = await api.request('/api/auth/state');
      if (!active()) return;
      final vault = VaultState.fromJson(await api.request('/api/vault/status'));
      if (!active()) return;
      if (auth['setup_required'] != false ||
          auth['recovery_required'] != false ||
          vault.hasError ||
          vault.configured && !vault.unlocked) {
        reset();
        preferences.reset();
        return;
      }
      final own = (await api.request('/api/status'))['node_id'];
      if (!active() ||
          own is! String ||
          !RegExp(r'^[a-f0-9]{64}$').hasMatch(own))
        return;
      final phones = ShellRepository.list(
        await api.request('/api/mobile/devices'),
        'devices',
      ).map((d) => d['peer_id']).toSet();
      if (!active()) return;
      final contacts = await ShellRepository(api).contacts();
      if (!active()) return;
      final trusted = ShellRepository.list(
        await api.request('/api/peers/trusted'),
        'peers',
      );
      if (!active()) return;
      final peers = <String>{};
      for (final c in contacts) {
        if (c['is_phone'] == true) continue;
        final short = c['short_id'];
        if (short is! String || !RegExp(r'^[a-f0-9]{16}$').hasMatch(short))
          continue;
        final ids = trusted.where(
          (t) => t['id'] is String && (t['id'] as String).startsWith(short),
        );
        if (ids.length != 1) continue;
        final id = ids.single['id'] as String;
        if (RegExp(r'^[a-f0-9]{64}$').hasMatch(id) && !phones.contains(id))
          peers.add(id);
        if (peers.length == 32) break;
      }
      baseline.removeWhere(
        (peer, _) => !peer.startsWith('group:') && !peers.contains(peer),
      );
      unread.removeWhere(
        (peer) => !peer.startsWith('group:') && !peers.contains(peer),
      );
      bool fresh = false;
      for (final peer in peers) {
        if (!active()) return;
        final messages = await ShellRepository(api).history(peer);
        if (!active()) return;
        final previous = baseline[peer];
        final ids = messages.map((m) => '${m['msg_id']}').take(100).toSet();
        num latest = previous?.latest ?? 0;
        for (final m in messages.take(100)) {
          final ts = m['timestamp'];
          if (ts is! num || m['msg_id'] is! String) continue;
          if (previous != null &&
              m['from'] == peer &&
              !isTombstone(m['text']) &&
              ts >= previous.latest &&
              !previous.ids.contains(m['msg_id']) &&
              peer != openPeer) {
            unread.add(peer);
            fresh = true;
          }
          if (ts > latest) latest = ts;
        }
        baseline[peer] = (ids: ids, latest: latest);
      }
      final groupRepo = GroupRepository(api);
      final groups = ShellRepository.list(
        await groupRepo.groups(),
        'groups',
      ).take(8);
      if (!active()) return;
      final groupPeers = <String>{};
      for (final g in groups) {
        final id = '${g['id']}', peer = 'group:${g['id']}';
        groupPeers.add(peer);
        final feed = await groupRepo.feed(id);
        if (!active()) return;
        if (feed['has_key'] != true ||
            feed['problem'] == true ||
            feed['damaged'] == true) {
          baseline.remove(peer);
          unread.remove(peer);
          continue;
        }
        final messages = ShellRepository.list(feed, 'messages');
        final previous = baseline[peer];
        num latest = previous?.latest ?? 0;
        final recent = messages.reversed.take(100).toList();
        for (final m in recent) {
          final ts = m['ts'] as num;
          if (previous != null &&
              m['mine'] == false &&
              !isTombstone(m['text']) &&
              ts >= previous.latest &&
              !previous.ids.contains('${m['seq']}') &&
              peer != openPeer) {
            unread.add(peer);
            fresh = true;
          }
          if (ts > latest) latest = ts;
        }
        baseline[peer] = (
          ids: recent.map((m) => '${m['seq']}').toSet(),
          latest: latest,
        );
      }
      baseline.removeWhere(
        (peer, _) => peer.startsWith('group:') && !groupPeers.contains(peer),
      );
      unread.removeWhere(
        (peer) => peer.startsWith('group:') && !groupPeers.contains(peer),
      );
      final finalVault = VaultState.fromJson(
        await api.request('/api/vault/status'),
      );
      if (!active()) return;
      if (finalVault.hasError ||
          finalVault.configured && !finalVault.unlocked) {
        reset();
        preferences.reset();
        return;
      }
      if (fresh && active()) await preferences.alert();
      if (active()) preferences.update();
    } catch (_) {
      // Do not keep stale baselines across unavailable/invalid daemon state.
      if (active()) {
        reset();
        preferences.reset();
      }
    } finally {
      busy = false;
    }
  }
}
