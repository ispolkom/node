import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../theme.dart';
import '../widgets/secure_field.dart';
import 'pair_screen.dart';
import 'qr_scan_screen.dart';

/// 64 hex-знака ID из любого текста (сообщение, JSON из QR, строка с пробелами или переносами).
String? extractPeerId(String raw) {
  final compact = raw.replaceAll(RegExp(r'[\s:]'), '');
  final m = RegExp(r'(?<![0-9a-fA-F])[0-9a-fA-F]{64}(?![0-9a-fA-F])').firstMatch(compact);
  return m?.group(0)?.toLowerCase();
}

/// Добавить контакт по ID: вставить из буфера, отсканировать QR или набрать своей клавиатурой.
class AddContactScreen extends StatefulWidget {
  final String initialId;
  const AddContactScreen({super.key, this.initialId = ''});
  @override
  State<AddContactScreen> createState() => _AddContactScreenState();
}

class _AddContactScreenState extends State<AddContactScreen> {
  late final TextEditingController _id = TextEditingController(text: widget.initialId);
  final TextEditingController _name = TextEditingController();
  String? _error;

  @override
  void dispose() {
    _id.dispose();
    _name.dispose();
    super.dispose();
  }

  Future<void> _scan() async {
    final raw = await Navigator.of(context).push<String>(MaterialPageRoute(builder: (_) => const QrScanScreen(title: 'QR контакта')));
    if (raw == null || !mounted) return;
    if (isPairingPayload(raw)) {
      // это QR сопряжения с узлом, а не ID контакта: открываем сопряжение, оно подключится само
      Navigator.of(context).pushReplacement(MaterialPageRoute(builder: (_) => PairScreen(initialPayload: raw)));
      return;
    }
    final id = extractPeerId(raw);
    setState(() {
      if (id == null) {
        _error = 'В этом QR нет ID контакта (64 знака).';
      } else {
        _error = null;
        _id.text = id;
      }
    });
  }

  Future<void> _pasteId() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text ?? '';
    if (isPairingPayload(text)) {
      if (mounted) Navigator.of(context).pushReplacement(MaterialPageRoute(builder: (_) => PairScreen(initialPayload: text)));
      return;
    }
    final id = extractPeerId(text);
    if (!mounted) return;
    setState(() {
      if (id == null) {
        _error = 'В буфере обмена нет ID (64 знака).';
      } else {
        _error = null;
        _id.text = id;
      }
    });
  }

  Future<void> _add() async {
    final id = extractPeerId(_id.text);
    if (id == null) {
      setState(() => _error = 'ID должен быть 64 знака 0-9 a-f.');
      return;
    }
    final state = context.read<AppState>();
    await state.addManualContact(id, _name.text.trim());
    if (mounted) Navigator.of(context).pop(true);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        title: const Text('Добавить контакт', style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: SingleChildScrollView(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(children: [
              Expanded(child: ElevatedButton.icon(onPressed: _scan, icon: const Icon(Icons.qr_code_scanner), label: const Text('Сканировать QR'))),
              const SizedBox(width: 8),
              Expanded(child: OutlinedButton.icon(onPressed: _pasteId, icon: const Icon(Icons.content_paste), label: const Text('ID из буфера'))),
            ]),
            const SizedBox(height: 16),
            SecureField(controller: _id, hint: 'ID контакта (64 знака)', minLines: 2, maxLines: 3, fontSize: 12),
            const SizedBox(height: 8),
            SecureField(controller: _name, hint: 'Имя (необязательно)', onSubmitted: _add),
            if (_error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(_error!, style: const TextStyle(color: Colors.redAccent))),
            const SizedBox(height: 12),
            ElevatedButton(onPressed: _add, child: const Text('Добавить')),
          ],
        ),
      ),
    );
  }
}
