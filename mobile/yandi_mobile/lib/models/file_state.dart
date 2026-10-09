/// Ход передачи файла: идёт (с долей от 0 до 1) или закончилась ошибкой.
class FileState {
  final double  progress;
  final String? error;
  const FileState.working(this.progress) : error = null;
  const FileState.failed(this.error) : progress = 0;
  bool get failed => error != null;
}
