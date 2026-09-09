#!/usr/bin/env bash
# Кладёт приватную часть ключа обновлений (см. updater-keypair.sh) в секреты репозитория GitHub,
# откуда её берёт release.yml. Значение в терминал не выводится. Пароль у ключа пустой.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

PRIV="$HOME/.tauri/findock-desktop.key"
[ -f "$PRIV" ] || { echo "нет $PRIV — сначала scripts/updater-keypair.sh" >&2; exit 1; }

REPO="$(git remote get-url origin | sed -E 's#.*github.com[:/]##; s#\.git$##')"
gh secret set TAURI_SIGNING_PRIVATE_KEY --repo "$REPO" < "$PRIV"
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo "$REPO" --body ""
echo "секреты записаны в $REPO"
gh secret list --repo "$REPO"
