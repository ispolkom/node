<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 9)',
        'date' => '2026-10-28',
        'changes' => [
            'ru' => [
                'Добавлены подтверждение доставки приглашения и ограниченные повторы при потере сигнала звонка.',
                'Сигналы звонков получают приоритет перед ожидающими записями переписки; отменённые звонки не открываются от запоздавшего приглашения.'
            ],
            'en' => [
                'Added invite delivery receipts and bounded retransmission after signal loss.',
                'Call signals take priority over pending history writes; delayed invitations cannot reopen canceled calls.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g898.',
            'en' => 'Built together with node 0.2.0+g898.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '123.3',
                'sha256' => '3e94b4a1244bfeed3bc79c21ea13f19e1760545853a87c1fb30d07e622ca857b'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g898',
        'date' => '2026-10-28',
        'changes' => [
            'ru' => [
                'Добавлены подтверждение получения вызова телефоном и ограниченные повторы приглашения.',
                'Дубликаты и запоздавшие приглашения отменённых звонков обрабатываются без повторного открытия окна.',
                'Узел сообщает об ошибках шифрования и отправки сигналов вместо ложного подтверждения отправки.'
            ],
            'en' => [
                'Added phone invite delivery receipts and bounded retransmission.',
                'Duplicate and delayed canceled-call invitations do not reopen the call window.',
                'The node reports encryption and signal send failures instead of false success.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android: 1.0.0, сборка 9.',
            'en' => 'Install both packages of the same version together. Android: 1.0.0 build 9.'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g898_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g898_amd64.deb',
                'size' => '30.7',
                'sha256' => '28e4d64d1da74c3430b1d6fc1eefc8e59802a06bcad439ef259258aa6822d480'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g898_amd64.deb',
                'name' => 'yandi-node_0.2.0-g898_amd64.deb',
                'size' => '7.0',
                'sha256' => '5cf388c18d76f1aa00de81ce3a576ae73fca49970e3268aaf29e2bfaa52397d0'
            ]
        ]
    ]
];
