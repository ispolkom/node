import 'package:flutter/material.dart';
import 'platform_files.dart';
import 'translations.dart';

/// Session-only preferences and bounded message baseline. No localStorage/files.
class AppPreferences extends ChangeNotifier {
  bool english = false,
      sound = false,
      notifications = false,
      dnd = false,
      background = false;
  String? failure;
  String? _peer;
  Set<String>? _seen;
  num _latest = 0;
  int generation = 0;
  bool disposed = false;
  @override
  void dispose() {
    disposed = true;
    generation++;
    super.dispose();
  }

  void update() {
    if (!disposed) notifyListeners();
  }

  void changed() {
    failure = null;
    notifyListeners();
  }

  void reset() {
    generation++;
    _latest = 0;
    _peer = null;
    _seen = null;
    PlatformFiles.channel
        .invokeMethod<void>('withdrawNotification')
        .catchError((Object _) {});
  }

  Future<void> observe(
    String peer,
    String? own,
    List<Map<String, dynamic>> messages,
  ) async {
    if (disposed) return;
    final samePeer = _peer == peer;
    final cutoff = samePeer ? _latest : 0;
    final ids = messages.map((m) => '${m['msg_id']}').toSet();
    final fresh =
        samePeer &&
        own != null &&
        _seen != null &&
        messages.any(
          (m) =>
              m['from'] != own &&
              m['timestamp'] is num &&
              (m['timestamp'] as num) >= cutoff &&
              m['msg_id'] is String &&
              !_seen!.contains(m['msg_id']),
        );
    _latest = cutoff;
    for (final m in messages) {
      final timestamp = m['timestamp'];
      if (timestamp is num && timestamp > _latest) _latest = timestamp;
    }
    _peer = peer;
    _seen = ids;
    if (!fresh) return;
    await alert();
  }

  Future<void> alert() async {
    final ownGeneration = generation;
    if (disposed || dnd || !sound && !notifications) return;
    try {
      await PlatformFiles.channel.invokeMethod<void>('messageAlert', {
        'sound': sound,
        'notify': notifications,
        'english': english,
      });
    } catch (_) {
      if (disposed || generation != ownGeneration) return;
      failure = 'Системное оповещение недоступно.';
      notifyListeners();
    }
  }
}

class PreferencesScope extends InheritedNotifier<AppPreferences> {
  const PreferencesScope({
    super.key,
    required AppPreferences preferences,
    required super.child,
  }) : super(notifier: preferences);
  static AppPreferences? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<PreferencesScope>()?.notifier;
}

const russianStates = <String, String>{
  'owner': 'владелец',
  'admin': 'админ',
  'moderator': 'модератор',
  'member': 'участник',
  'new': 'новое устройство',
  'hidden': 'скрыто',
  'blocked': 'заблокировано',
  'unknown': 'неизвестно',
  'disabled': 'выключено',
  'cooldown': 'пауза',
  'ok': 'работает',
};
String stateLabel(BuildContext context, dynamic value) =>
    PreferencesScope.of(context)?.english == true
    ? '$value'
    : russianStates['$value'] ?? '$value';

String tr(BuildContext context, String text) =>
    translate(text, english: PreferencesScope.of(context)?.english == true);

final sortedEnglishTemplates = englishTemplates.entries.toList()
  ..sort(
    (a, b) => b.key
        .replaceAll(RegExp(r'\{[0-9]+\}'), '')
        .length
        .compareTo(a.key.replaceAll(RegExp(r'\{[0-9]+\}'), '').length),
  );

String translate(String text, {required bool english}) {
  if (!english) return text;
  final exact = englishLabels[text];
  if (exact != null) return exact;
  for (final e in sortedEnglishTemplates) {
    final markers = RegExp(r'\{([0-9]+)\}').allMatches(e.key).toList();
    var pattern = '^', pos = 0;
    for (final m in markers) {
      pattern += RegExp.escape(e.key.substring(pos, m.start)) + '(.*?)';
      pos = m.end;
    }
    pattern += RegExp.escape(e.key.substring(pos)) + r'$';
    final match = RegExp(pattern, dotAll: true).firstMatch(text);
    if (match == null) continue;
    return e.value.replaceAllMapped(RegExp(r'\{([0-9]+)\}'), (m) {
      final index = int.parse(m[1]!);
      final value = match.group(index + 1) ?? '';
      // Translate only known state/label slots; names and message text stay verbatim.
      final stateSlot =
          e.key == 'Запретить {0}' ||
          e.key == 'Подтвердить: {0}?' ||
          e.key == 'Статус демона: {0}' ||
          e.key == 'Сервер: {0}' ||
          e.key == 'Запрещено: {0}' ||
          e.key ==
              'Соединение: {0} · прокси по данным демона: {1} · задержка: —' ||
          e.key == '{0} · {1} · изменено' && index == 1;
      if (stateSlot)
        return value.split(', ').map((v) => englishLabels[v] ?? v).join(', ');
      return value;
    });
  }
  return text;
}

class UiText extends StatelessWidget {
  final String text;
  final TextStyle? style;
  const UiText(this.text, {super.key, this.style});
  @override
  Widget build(BuildContext context) => Text(tr(context, text), style: style);
}

class UiSelectableText extends StatelessWidget {
  final String text;
  const UiSelectableText(this.text, {super.key});
  @override
  Widget build(BuildContext context) => SelectableText(tr(context, text));
}

const englishLabels = <String, String>{
  "Входной список узлов": "Entry node list",
  "Ключ подписи списка хранится только в закрытой среде узла и никуда не передаётся; подписывает сам узел (yandi admin bootstrap sign).": "The list signing key is kept only in the node’s closed environment and is never transmitted; the node signs it itself (yandi admin bootstrap sign).",
  "Закрытая среда заперта: ключ подписи недоступен.": "The closed environment is locked: the signing key is unavailable.",
  "Скопировать открытый ключ": "Copy the public key",
  "Заявки узлов": "Node applications",
  "Узлы с белым IP сами присылают данные подключения тем, кто в списке. Заявка ничего не добавляет: решаете вы.": "Nodes with a public IP send their connection data to those in the list on their own. An application adds nothing: you decide.",
  "Обновить заявки": "Refresh applications",
  "Заявок нет.": "No applications.",
  "Новая: адреса ещё прозваниваются": "New: the addresses are still being probed",
  "Проверена: адрес ответил ключом узла": "Verified: an address answered with the node key",
  "Не отвечает по заявленным адресам": "Does not answer at the declared addresses",
  "Отклонена": "Declined",
  "Уже в списке": "Already in the list",
  "Неизвестный статус": "Unknown status",
  "Скопировать команду для списка": "Copy the list command",
  'Скопировать код': 'Copy code',
  'Скопировано': 'Copied',
  ...moreEnglishLabels,
  'Панель': 'Dashboard',
  'Главная': 'Home',
  'Ошибка': 'Error',
  'Управление': 'Control',
  'Добавить': 'Add',
  'Мои группы': 'My groups',
  'Список групп': 'Group list',
  'Настройки оболочки': 'Shell settings',
  'Управление Relay Server': 'Relay server control',
  'Активные Relay Сессии': 'Active relay sessions',
  'Подключение через Relay': 'Connect via relay',
  'Выберите группу слева.': 'Choose a group on the left.',
  'Группы. Группы телефонов со сквозным шифрованием: компьютер участвует как обычное устройство. Содержимое доступно только участникам и хранится на узле в зашифрованном виде.':
      'Groups. End-to-end encrypted phone groups: this computer joins as a regular device. Content is available only to members and is stored encrypted on the node.',
  'Контакты': 'Contacts',
  'Чат': 'Chat',
  'Группы': 'Groups',
  'Шлюзы': 'Gateways',
  'Звонки': 'Calls',
  'Видеозвонок': 'Video call',
  'Все': 'All',
  'Смотреть': 'Watch',
  'Слушать': 'Listen',
  'Остановить': 'Stop',
  'Эмодзи': 'Emoji',
  'Новая просьба': 'New request',
  'просит добавить его в группу или прислать приглашение':
      'asks to be added to the group or sent an invitation',
  'служебное: видят владелец и модераторы':
      'service: visible to the owner and moderators',
  'Выбрать': 'Select',
  'Редактирование сообщения': 'Editing message',
  'Удалить выбранные': 'Delete selected',
  'Снять выбор': 'Clear selection',
  'Приглашение в группу': 'Group invitation',
  'Приглашение отправлено (действует сутки, один раз)':
      'Invitation sent (valid for a day, once)',
  'Картинки': 'Pictures',
  'В этом разделе пусто': 'Nothing in this section',
  'Настройки': 'Settings',
  'Закрытая среда': 'Private vault',
  'YANDI · локальная оболочка': 'YANDI · local shell',
  'Отмена': 'Cancel',
  'Сохранить': 'Save',
  'Удалить': 'Delete',
  'Подтвердить': 'Confirm',
  'Продолжить': 'Continue',
  'Без секретных слов': 'Without recovery words',
  'Состояние блокировки не подтверждено': 'Lock state is unconfirmed',
  'Нет связи с демоном': 'Daemon disconnected',
  'Статус узла': 'Node status',
  'Загрузка статуса…': 'Loading status…',
  'Скорость': 'Speed',
  'Загрузка метрик…': 'Loading metrics…',
  'Узлы': 'Nodes',
  'Обнаруженных узлов нет.': 'No nodes discovered.',
  'Подключение узла в текущем API — заглушка.':
      'Node connection is a stub in the current API.',
  'Проверка закрытой среды…': 'Checking private vault…',
  'Сохранить автоблокировку': 'Save auto-lock',
  'Участники': 'Members',
  'Повреждена лента на диске.': 'The feed on disk is damaged.',
  'Ключ владельца изменился: доверие не подтверждено.':
      'Owner key changed: trust is unconfirmed.',
  'Ждём ключ группы.': 'Waiting for the group key.',
  'Вам запрещено писать.': 'You cannot post.',
  'История пуста.': 'History is empty.',
  'Отправить группе': 'Send to group',
  'Переслать': 'Forward',
  'Переслать выбранные': 'Forward selected',
  'Переслать сообщение': 'Forward message',
  'Файлы чата': 'Chat files',
  'Сохранить контакт': 'Save contact',
  'Поделиться контактом': 'Share a contact',
  'Контакт': 'Contact',
  'Контакт отправлен': 'Contact sent',
  'Уже в ваших контактах': 'Already in your contacts',
  'Контакт от собеседника': 'Contact from the other person',
  'Нет контактов, которыми можно поделиться.': 'No contacts to share.',
  'Собеседник получит карточки с адресом и именем и сам решит, сохранять ли их. В группы контакты не передаются.': 'The other person will get cards with the address and name and decides whether to save them. Contacts are never sent to groups.',
  'Вся цепочка ответов': 'Whole reply chain',
  'Цепочка ответов: сверху — с чего всё началось': 'Reply chain: the first message is at the top',
  'Заглушить': 'Mute',
  'Смайл…': 'Emoji…',
  'Смайл на сообщение': 'Emoji for the message',
  '🪙 Одобряю': '🪙 Approve',
  '🪙 Не одобряю': '🪙 Disapprove',
  'Голосовое сообщение': 'Voice message',
  'Отправить голосовое': 'Send voice message',
  'Идёт запись…': 'Recording…',
  'Некому отправлять: в списке нет контактов.': 'Nobody to send to: no contacts.',
  'Переслать запись': 'Forward entry',
  'Файлы группы': 'Group files',
  'Назад к сообщениям': 'Back to messages',
  'Вы ещё не состоите в группах.': 'You have not joined any groups.',
  'Каталог пуст.': 'Catalog is empty.',
  'Просьба отправлена': 'Request sent',
  'Правила группы': 'Group rules',
  'Сохранить правила': 'Save rules',
  'Контактов пока нет.': 'No contacts yet.',
  'Открыть чат': 'Open chat',
  'Переименовать': 'Rename',
  'Картинка': 'Picture',
  'Как он сам': 'As he set it',
  'Снять локальные имя и картинку?': 'Remove local name and picture?',
  'Контакт снова будет показываться так, как он назвал себя сам.': 'The contact will be shown the way he named himself again.',
  'Картинка до 2 МБ.': 'Picture up to 2 MB.',
  'Это не картинка (нужны PNG, JPEG или GIF).': 'This is not a picture (PNG, JPEG or GIF needed).',
  'Отозвать доверие': 'Revoke trust',
  'Удалить контакт': 'Delete contact',
  'Выберите контакт для переписки.': 'Choose a contact to chat with.',
  'Очистить историю': 'Clear history',
  'Показаны последние 100 сообщений не старше года.':
      'Last 100 messages from the past year.',
  'Изменить сообщение': 'Edit message',
  'Удалить сообщение': 'Delete message',
  'Выбрать файл': 'Choose file',
  'Убрать файл': 'Remove file',
  'Отправить файл': 'Send file',
  'Отправить': 'Send',
  'Данные не обновлены.': 'Data is not current.',
  'Профиль': 'Profile',
  'Сохранить профиль': 'Save profile',
  'Карточка собственного узла': 'Own node card',
  'Копировать карточку': 'Copy card',
  'Экспорт contacts.json': 'Export contacts.json',
  'Добавить по карточке узла': 'Add using node card',
  'Добавить контакт': 'Add contact',
  'Не удалось показать картинку': 'Cannot display image',
  'Проверить файл': 'Check file',
  'Скачать': 'Download',
  'Предпросмотр': 'Preview',
  'Сохранить и открыть': 'Save and open',
  'Запустить туннель': 'Start tunnel',
  'Остановить туннель': 'Stop tunnel',
  'Статус туннеля': 'Tunnel status',
  'Удалить аватар профиля?': 'Remove profile avatar?',
  'Удалить аватар': 'Remove avatar',
  'Не удалось показать аватар': 'Cannot display avatar',
  'Обновить профиль': 'Refresh profile',
  'Изменить имя и аватар': 'Change name and avatar',
  'Просмотреть аватар': 'View avatar',
  'Загрузить аватар': 'Upload avatar',
  'Удалить аватар профиля': 'Remove profile avatar',
  'Входящие проверяются, пока открыт этот экран.':
      'Incoming calls are checked while this screen is open.',
  'Уровень микрофона': 'Microphone level',
  'Собеседник': 'Peer',
  'Вы': 'You',
  'Телефон': 'Phone',
  'Видео': 'Video',
  'Язык / Language': 'Language / Язык',
  'Системный звук': 'System sound',
  'Уведомления ОС': 'OS notifications',
  'Не беспокоить': 'Do not disturb',
  'Оповещения только о новых сообщениях открытого чата. Содержимое и имя в ОС не передаются. Настройки действуют до закрытия оболочки.':
      'Alerts cover new messages in the open chat. Content and names are never sent to the OS. Preferences last until the shell closes.',
  'Английский перевод частичный: меню и основные кнопки. Формы и подробные состояния пока на русском.':
      'English translation is partial: navigation and main buttons. Forms and detailed states remain in Russian.',
};
