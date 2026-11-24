<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 10)',
        'date' => '2026-11-25',
        'changes' => [
            'ru' => [
                'Исправлена фоновая доставка сигналов входящих звонков с подтверждением после E2E-обработки.',
                'Повторные приглашения и несколько одновременных вызовов обрабатываются с сохранением состояния звонка.'
            ],
            'en' => [
                'Background incoming-call signals are acknowledged after E2E processing.',
                'Repeated invitations and concurrent calls preserve call state.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g961.',
            'en' => 'Built together with node 0.2.0+g961.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '130.6',
                'sha256' => '840f893413c22ed421091c71bda4fcca6cceb4559da3fad59c2708c12abc9820'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g961',
        'date' => '2026-11-25',
        'changes' => [
            'ru' => [
                'Оболочка продолжает проверять звонки, когда её окно находится в фоне.',
                'Исправлены зависимости пакетов и запуск пользовательской службы после установки и обновления.',
                'Закрыты ошибки хранения очереди сообщений и блокировки личной среды при сбоях записи и смене ключей.'
            ],
            'en' => [
                'The desktop shell continues polling for calls while its window is in the background.',
                'Package dependencies and user service startup after install and upgrade were corrected.',
                'Fixed message queue preservation and private environment lock behavior during failed writes and key changes.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android: 1.0.0, сборка 10.',
            'en' => 'Install both packages of the same version together. Android: 1.0.0 build 10.'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g961_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g961_amd64.deb',
                'size' => '32.2',
                'sha256' => 'f73feddc518898d7dc04016bda0b5d61f3ef062892b082cc49996d340be15e32'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g961_amd64.deb',
                'name' => 'yandi-node_0.2.0-g961_amd64.deb',
                'size' => '7.3',
                'sha256' => '3ee7f894993079d6f8520f490f2a90fcf7b6828089b75ed8f4ee6b455ad602f1'
            ]
        ]
    ]
];
