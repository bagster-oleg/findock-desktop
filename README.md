# FinDock для компьютера

Тонкая оболочка над кабинетом [cp.findock.ru](https://cp.findock.ru): своё окно с иконкой в доке
и Пуске, сессия между запусками, скачивание документов сразу в «Загрузки», тихое автообновление.
Всё остальное — сайт внутри окна: новые функции появляются на сервере, приложение обновлять не надо.

Стек: [Tauri 2](https://v2.tauri.app) (Rust + системный webview: WKWebView на Mac, WebView2 на
Windows). Веб-фронта здесь нет — `ui/index.html` пустая заглушка для сборщика.

## Что делает оболочка (`src-tauri/src/lib.rs`)

- Открывает кабинет (`APP_URL`, переопределяется `FINDOCK_URL=https://dev.findock.ru/` при сборке).
- Скачивание → «Загрузки» пользователя, имя от сайта, при совпадении `(1)`, `(2)`…; готовый файл
  показывается в Finder/Проводнике.
- В окне остаются `*.findock.ru` и страницы входа провайдеров подключений (`*.google.com`,
  `*.yandex.ru`). Остальные адреса и `target=_blank` — в системный браузер.
- Один экземпляр: повторный запуск поднимает открытое окно.
- При старте — проверка `latest.json` в последнем релизе, тихая загрузка и установка обновления.
  Windows: установщик в passive-режиме сам перезапускает приложение. Mac: подмена бандла и
  `restart()`.

## Сборка

```
npm ci
npm run build:mac        # universal .dmg → src-tauri/target/universal-apple-darwin/release/bundle/dmg
npm run build            # текущая платформа (на Windows — NSIS .exe)
npm run dev              # окно в режиме разработки
FINDOCK_URL=https://dev.findock.ru/ npm run build:mac   # сборка на dev-стенд
```

Нужны Rust (`rustup`, targets `aarch64-apple-darwin` + `x86_64-apple-darwin` для универсальной
сборки) и Node 22. Иконки: `npm run icons` из `assets/app-icon.png` (1024×1024, орб FinDock).

## Релиз

Тег `vX.Y.Z` (версия в `src-tauri/Cargo.toml` и `package.json` должна совпадать) запускает
[release.yml](.github/workflows/release.yml): Mac + Windows → GitHub Release с версионными
файлами, `latest.json` для автообновления и копиями со стабильными именами:

```
https://github.com/bagster-oleg/findock-desktop/releases/latest/download/FinDock.dmg
https://github.com/bagster-oleg/findock-desktop/releases/latest/download/FinDock-setup.exe
```

На них ведут постоянные ссылки `cp.findock.ru/download/mac` и `/download/windows`.

### Подпись обновлений

Обновления подписаны ключом minisign (встроен в Tauri). Пара создаётся один раз
`scripts/updater-keypair.sh`: приватная часть — `~/.tauri/findock-desktop.key` и секрет репозитория
`TAURI_SIGNING_PRIVATE_KEY` (`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — пустой), публичная —
`plugins.updater.pubkey` в `src-tauri/tauri.conf.json`. **Потеря приватной части = установленные
приложения перестанут принимать обновления**, бэкап обязателен.

### Без подписи кода Apple/Microsoft

Юрлица и платных аккаунтов нет, поэтому:
- Mac: ad-hoc подпись (`signingIdentity: "-"`). Скачанный `.dmg` при первом запуске: Настройки →
  Конфиденциальность и безопасность → «Открыть всё равно».
- Windows: SmartScreen «Система защитила ваш компьютер» → «Подробнее» → «Выполнить в любом случае».

Это один раз при установке. Обновления приложение качает само, без браузера — карантина и
предупреждений на них нет.
