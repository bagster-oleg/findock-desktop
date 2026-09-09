#!/usr/bin/env bash
# Пара ключей для подписи обновлений (minisign, встроен в Tauri). Приватная часть — только в
# ~/.tauri/findock-desktop.key (и в секрет GitHub TAURI_SIGNING_PRIVATE_KEY), в чат и репо не попадает.
# Публичная часть вписывается в src-tauri/tauri.conf.json → plugins.updater.pubkey.
# Повторный запуск с существующим файлом ничего не перегенерирует: потеря приватной части =
# все установленные приложения перестанут принимать обновления.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

PRIV="$HOME/.tauri/findock-desktop.key"
PUB="$PRIV.pub"
mkdir -p "$HOME/.tauri"

if [ ! -f "$PRIV" ]; then
    npx tauri signer generate -w "$PRIV" --ci >/dev/null 2>&1
    echo "пара создана: $PRIV(.pub)"
else
    echo "пара уже есть: $PRIV"
fi

PUBKEY="$(cat "$PUB")"
python3 - "$PUBKEY" <<'EOF'
import sys, json
p = 'src-tauri/tauri.conf.json'
cfg = json.load(open(p))
cfg['plugins']['updater']['pubkey'] = sys.argv[1]
json.dump(cfg, open(p, 'w'), ensure_ascii=False, indent=2)
open(p, 'a').write('\n')
print('публичный ключ записан в tauri.conf.json, длина', len(sys.argv[1]))
EOF
