<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 7)',
        'date' => '2026-10-27',
        'changes' => [
            'ru' => [
                'Исправлен звук личных сообщений в открытом чате и фоновый звук icq.mp3 без зависимости от системной мелодии.',
                'Сохранённые контакты доступны без интернета после разблокировки; история и файлы читаются из локального хранилища.',
                'Исправлена ошибка Gson/R8 при снятии уведомлений.'
            ],
            'en' => [
                'Fixed message sounds in the open chat and bundled icq.mp3 background alerts independent of the system notification tone.',
                'Saved contacts are restored offline after unlocking; history and files use local storage.',
                'Fixed the Gson/R8 notification cancellation error.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g891. Используйте пакеты этого выпуска.',
            'en' => 'Built with node 0.2.0+g891. Use matching release packages.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '123.3',
                'sha256' => '2186827ffebe1aa4e4044089477bc4e6c27753e638c5f9c01335de8264645e91'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g891',
        'date' => '2026-10-27',
        'changes' => [
            'ru' => [
                'Исправлены оповещения сообщений от телефонов без входа в админку; фоновая проверка больше не сбрасывает состояние открытого чата.',
                'Звонок сохраняется при потере фокуса окна. Исправлено закрытие окна звонка после завершения.',
                'Узел и оболочка собраны из одного коммита; контрольные суммы приведены для каждого пакета.'
            ],
            'en' => [
                'Fixed phone message alerts without an admin session; background polling no longer resets open-chat sound state.',
                'Calls survive focus changes. Fixed call window cleanup after hangup.',
                'Node and desktop share one source commit; SHA-256 is listed for each package.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android: 1.0.0 (сборка 7).',
            'en' => 'Install both packages of the same version together. Android: 1.0.0 (build 7).'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g891_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g891_amd64.deb',
                'size' => '30.7',
                'sha256' => 'edd16d1ac442b22ebb992dae1d7ff13f309ad19fff71738b2a467d5da74722ba'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g891_amd64.deb',
                'name' => 'yandi-node_0.2.0-g891_amd64.deb',
                'size' => '7.0',
                'sha256' => 'd33e990326a269eecf4b8f159801805e4659b5c81f754d9354645d55f49d52a9'
            ]
        ]
    ]
];
