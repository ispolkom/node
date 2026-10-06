import 'dart:convert';
import 'dart:io';
import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:http/io_client.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:provider/provider.dart';
import '../models/trusted_node.dart';
import '../services/app_state.dart';
import '../theme.dart';

/// QR-формат: JSON {"host":"...","port":8766,"pairing_code":"123456","tls_fingerprint":"...","tls":true}
class PairScreen extends StatefulWidget {
  const PairScreen({super.key});
  @override
  State<PairScreen> createState() => _PairScreenState();
}

class _PairScreenState extends State<PairScreen> {
  bool _processing = false;
  String? _error;

  final MobileScannerController _scanner = MobileScannerController(
    detectionSpeed: DetectionSpeed.noDuplicates,
    facing: CameraFacing.back,
  );

  @override
  void dispose() {
    _scanner.dispose();
    super.dispose();
  }

  Future<void> _onQr(String raw) async {
    if (_processing) return;
    setState(() { _processing = true; _error = null; });

    try {
      final data        = jsonDecode(raw) as Map<String, dynamic>;
      final host        = data['host']            as String;
      final port        = data['port']            as int;
      final code        = data['pairing_code']    as String;
      final fingerprint = data['tls_fingerprint'] as String? ?? '';
      final useTls      = data['tls']             as bool? ?? false;
      final scheme      = useTls ? 'https' : 'http';

      // Pinned HTTP client для паринга (fingerprint известен из QR)
      final client = _buildPinnedClient(fingerprint);

      final res = await client.post(
        Uri.parse('$scheme://$host:$port/mobile/pair'),
        headers: {'Content-Type': 'application/json'},
        body: jsonEncode({
          'pairing_code': code,
          'device_name':  'YANDI Mobile',
        }),
      ).timeout(const Duration(seconds: 10));

      if (res.statusCode != 200) throw Exception('Pairing failed: ${res.body}');

      final body  = jsonDecode(res.body) as Map<String, dynamic>;
      final token = body['token'] as String;

      // Получаем info чтобы узнать node_id
      final infoRes = await client.get(
        Uri.parse('$scheme://$host:$port/mobile/info'),
        headers: {
          'Content-Type':  'application/json',
          'Authorization': 'Bearer $token',
        },
      ).timeout(const Duration(seconds: 10));
      client.close();

      final info   = jsonDecode(infoRes.body) as Map<String, dynamic>;
      final nodeId = info['node_id'] as String? ?? '';

      final node = TrustedNode(
        id:          nodeId.isNotEmpty ? nodeId : '$host:$port',
        name:        info['name'] as String? ?? host,
        host:        host,
        port:        port,
        fingerprint: fingerprint,
        token:       token,
        addedAt:     DateTime.now().millisecondsSinceEpoch,
      );
      if (!mounted) return;
      await context.read<AppState>().completePairing(node);
      if (!mounted) return;
      Navigator.of(context).pushReplacementNamed('/home');
    } catch (e) {
      setState(() { _processing = false; _error = e.toString(); });
      _scanner.start();
    }
  }

  static http.Client _buildPinnedClient(String expectedFp) {
    final ctx = SecurityContext(withTrustedRoots: false);
    final ioClient = HttpClient(context: ctx)
      ..badCertificateCallback = (X509Certificate cert, String host, int port) {
          if (expectedFp.isEmpty) return true;
          final fp = crypto.sha256.convert(cert.der).toString();
          return fp.toLowerCase() == expectedFp.toLowerCase();
        };
    return IOClient(ioClient);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        title: const Text('Подключение к ноде', style: TextStyle(color: AppTheme.text)),
        automaticallyImplyLeading: false,
      ),
      body: Column(
        children: [
          const SizedBox(height: 24),
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 24),
            child: Text(
              'Откройте веб-интерфейс ноды и нажмите\n"Подключить мобильное устройство".\nПосканируйте QR-код.',
              textAlign: TextAlign.center,
              style: TextStyle(color: AppTheme.textSecondary, fontSize: 14),
            ),
          ),
          const SizedBox(height: 24),
          Expanded(
            child: _processing
                ? const Center(child: CircularProgressIndicator(color: AppTheme.accent))
                : ClipRRect(
                    borderRadius: BorderRadius.circular(16),
                    child: MobileScanner(
                      controller: _scanner,
                      onDetect: (capture) {
                        final raw = capture.barcodes.firstOrNull?.rawValue;
                        if (raw != null) _onQr(raw);
                      },
                    ),
                  ),
          ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.all(16),
              child: Text(_error!, style: const TextStyle(color: Colors.redAccent)),
            ),
          const SizedBox(height: 32),
        ],
      ),
    );
  }
}
