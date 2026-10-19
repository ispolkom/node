import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/contact.dart';
import '../services/app_state.dart';
import '../theme.dart';
import 'avatar_circle.dart';
import '../l10n.dart';

/// «Хотят познакомиться»: люди не из ваших контактов, которые вам написали. Принять, заблокировать (только у вас) или игнорировать.
class IntroRequests extends StatelessWidget {
  final List<Contact> peers;
  final void Function(Contact) onAccept;
  const IntroRequests({super.key, required this.peers, required this.onAccept});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    return Container(
      color: AppTheme.surface,
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 6),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(trf('Хотят познакомиться ({n})', {'n': peers.length}), style: const TextStyle(color: AppTheme.accent, fontWeight: FontWeight.w700, fontSize: 13)),
        const SizedBox(height: 6),
        for (final c in peers) _card(context, state, c),
      ]),
    );
  }

  Widget _card(BuildContext context, AppState state, Contact c) {
    final msgs = state.messagesFor(c.peerId).where((m) => !m.outgoing).toList();
    final preview = msgs.isEmpty ? '' : msgs.last.text.replaceAll(RegExp(r'\s+'), ' ');
    return Card(
      color: AppTheme.bg,
      margin: const EdgeInsets.only(bottom: 8),
      child: Padding(
        padding: const EdgeInsets.all(10),
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Row(children: [
            AvatarCircle(peerId: c.peerId, label: c.displayName),
            const SizedBox(width: 10),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(c.displayName, style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.w600)),
                if (preview.isNotEmpty)
                  Text(preview.length > 120 ? '${preview.substring(0, 120)}…' : preview,
                      style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13)),
              ]),
            ),
          ]),
          const SizedBox(height: 6),
          Wrap(spacing: 8, children: [
            FilledButton(
              style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black, visualDensity: VisualDensity.compact),
              onPressed: () => onAccept(c),
              child: Text(tr('Принять')),
            ),
            OutlinedButton(
              style: OutlinedButton.styleFrom(visualDensity: VisualDensity.compact),
              onPressed: () => state.dismissIntro(c.peerId),
              child: Text(tr('Игнорировать')),
            ),
            TextButton(
              onPressed: () async {
                final ok = await showDialog<bool>(
                  context: context,
                  builder: (ctx) => AlertDialog(
                    backgroundColor: AppTheme.surface,
                    title: Text(tr('Заблокировать?'), style: TextStyle(color: AppTheme.text)),
                    content: Text(tr('Он не сможет вам писать и видеть вас в сети. Для остальных на узле ничего не меняется.'),
                        style: TextStyle(color: AppTheme.textSecondary)),
                    actions: [
                      TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'))),
                      TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Заблокировать'), style: TextStyle(color: Colors.redAccent))),
                    ],
                  ),
                );
                if (ok == true) await state.blockContact(c.peerId, c.displayName);
              },
              child: Text(tr('Заблокировать'), style: TextStyle(color: Colors.redAccent)),
            ),
          ]),
        ]),
      ),
    );
  }
}
