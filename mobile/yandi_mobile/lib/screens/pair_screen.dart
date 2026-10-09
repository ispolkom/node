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
  final TextEditingController _manual = TextEditingController();

  // Каждый кадр с кодом доходит до onDetect: решение принимаем сами (код целиком
  // в кадре, крупный, читается несколько кадров подряд), а не по первому попавшемуся.
  final MobileScannerController _scanner = MobileScannerController(
    detectionSpeed: DetectionSpeed.normal,
    detectionTimeoutMs: 200,
    facing: CameraFacing.back,
  );

  static const int _needHits = 4;                       // подряд одинаковых кадров
  static const Duration _needStable = Duration(milliseconds: 900);
  static const Duration _gapReset = Duration(milliseconds: 700);
  String? _candidate;
  int _hits = 0;
  DateTime? _firstHit;
  DateTime? _lastHit;
  String _hint = 'Наведите камеру на QR: код должен целиком поместиться в рамку';
  bool _recognized = false;

  @override
  void dispose() {
    _scanner.dispose();
    _manual.dispose();
    super.dispose();
  }

  /// Понятное объяснение вместо сырого текста исключения.
  String _humanError(Object e) {
    final t = e.toString();
    if (t.contains('bad or expired pairing code')) {
      return 'Код устарел или уже использован. На компьютере нажмите «Показать QR» ещё раз и отсканируйте новый (код живёт 5 минут и подходит один раз).';
    }
    if (e is FormatException) {
      return 'Это не QR узла YANDI или текст вставлен не полностью. Вставьте весь текст из-под QR целиком.';
    }
    if (t.contains('TimeoutException')) {
      return 'Узел не ответил за 10 секунд. Проверьте адрес в QR, интернет на телефоне и что порт узла открыт.';
    }
    return t;
  }

  void _resetScan([String? hint]) {
    _candidate = null; _hits = 0; _firstHit = null; _lastHit = null;
    if (hint != null && hint != _hint && mounted) setState(() => _hint = hint);
  }

  /// Весь ли квадрат кода внутри кадра и достаточно ли он крупный для чёткого чтения.
  /// Углы приходят в системе координат кадра камеры (она может быть повёрнута), поэтому
  /// допускаем оба варианта ориентации.
  String? _geometryProblem(Barcode b, Size image) {
    final c = b.corners;
    if (c.length < 4 || image.isEmpty) return null; // углов нет — полагаемся на стабильность
    double minX = c.first.dx, maxX = c.first.dx, minY = c.first.dy, maxY = c.first.dy;
    for (final p in c) {
      if (p.dx < minX) minX = p.dx;
      if (p.dx > maxX) maxX = p.dx;
      if (p.dy < minY) minY = p.dy;
      if (p.dy > maxY) maxY = p.dy;
    }
    bool inside(double w, double h) {
      final mx = w * 0.03, my = h * 0.03;
      return minX >= mx && minY >= my && maxX <= w - mx && maxY <= h - my;
    }
    if (!inside(image.width, image.height) && !inside(image.height, image.width)) {
      return 'Код виден не весь: отодвиньте телефон, чтобы весь квадрат был в кадре';
    }
    final side = (maxX - minX) < (maxY - minY) ? (maxX - minX) : (maxY - minY);
    final shortImage = image.width < image.height ? image.width : image.height;
    if (side < shortImage * 0.22) return 'Код слишком мелкий: подвиньте телефон ближе';
    return null;
  }

  Future<void> _onDetect(BarcodeCapture capture) async {
    if (_processing || _recognized) return;
    final b = capture.barcodes.firstOrNull;
    final raw = b?.rawValue;
    final now = DateTime.now();
    if (b == null || raw == null || raw.isEmpty) {
      if (_lastHit != null && now.difference(_lastHit!) > _gapReset) {
        _resetScan('Наведите камеру на QR: код должен целиком поместиться в рамку');
      }
      return;
    }
    final problem = _geometryProblem(b, capture.size);
    if (problem != null) { _resetScan(problem); return; }

    if (raw != _candidate || (_lastHit != null && now.difference(_lastHit!) > _gapReset)) {
      _candidate = raw; _hits = 0; _firstHit = now;
    }
    _hits++;
    _lastHit = now;
    if (_hits < _needHits || now.difference(_firstHit!) < _needStable) {
      if (mounted) setState(() => _hint = 'Держите ровно, распознаю…');
      return;
    }
    // код целиком и стабильно: показываем это, даём секунду увидеть, и только потом подключаемся
    _recognized = true;
    if (mounted) setState(() => _hint = 'Код распознан. Подключаюсь…');
    await Future<void>.delayed(const Duration(milliseconds: 800));
    if (!mounted) return;
    await _onQr(raw);
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
      _recognized = false;
      _resetScan('Не вышло подключиться. Наведите камеру на QR ещё раз');
      if (!mounted) return;
      setState(() { _processing = false; _error = _humanError(e); });
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
              'Откройте настройки ноды, карточка\n«Приложение на телефоне», «Показать QR».\nПосканируйте QR-код или вставьте текст под ним.',
              textAlign: TextAlign.center,
              style: TextStyle(color: AppTheme.textSecondary, fontSize: 14),
            ),
          ),
          const SizedBox(height: 24),
          Expanded(
            child: ClipRRect(
                    borderRadius: BorderRadius.circular(16),
                    child: Stack(
                      fit: StackFit.expand,
                      children: [
                        MobileScanner(
                          controller: _scanner,
                          onDetect: _onDetect,
                        ),
                        // рамка-прицел: в неё должен целиком поместиться квадрат QR
                        Center(
                          child: FractionallySizedBox(
                            widthFactor: 0.7,
                            child: AspectRatio(
                              aspectRatio: 1,
                              child: DecoratedBox(
                                decoration: BoxDecoration(
                                  border: Border.all(
                                    color: _recognized ? Colors.greenAccent : AppTheme.accent,
                                    width: 3,
                                  ),
                                  borderRadius: BorderRadius.circular(12),
                                ),
                              ),
                            ),
                          ),
                        ),
                        Positioned(
                          left: 16, right: 16, bottom: 12,
                          child: Container(
                            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                            decoration: BoxDecoration(
                              color: Colors.black54,
                              borderRadius: BorderRadius.circular(10),
                            ),
                            child: Text(
                              _hint,
                              textAlign: TextAlign.center,
                              style: const TextStyle(color: Colors.white, fontSize: 14),
                            ),
                          ),
                        ),
                        if (_processing)
                          const Positioned.fill(
                            child: ColoredBox(
                              color: Colors.black54,
                              child: Center(child: CircularProgressIndicator(color: AppTheme.accent)),
                            ),
                          ),
                      ],
                    ),
                  ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
            child: Row(children: [
              Expanded(
                child: TextField(
                  controller: _manual,
                  style: const TextStyle(color: AppTheme.text, fontSize: 12),
                  decoration: const InputDecoration(hintText: 'Текст из QR (если камера не читает)'),
                ),
              ),
              const SizedBox(width: 8),
              ElevatedButton(
                onPressed: _processing
                    ? null
                    : () {
                        final t = _manual.text.trim();
                        if (t.isEmpty) {
                          setState(() => _error = 'Поле пустое: вставьте текст из-под QR на странице узла или отсканируйте QR камерой.');
                          return;
                        }
                        _onQr(t);
                      },
                child: const Text('Подключить'),
              ),
            ]),
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
