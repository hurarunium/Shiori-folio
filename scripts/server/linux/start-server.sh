#!/bin/sh
# Ver7.0 サーバー(Linux): 窓口(shiori-server)を起動する。systemdのユニットから呼ぶ。
# 例: ./start-server.sh                       (Tailscaleのアドレス:8090で待ち受ける)
#     BIND=127.0.0.1:8090 ./start-server.sh   (手元だけで試すとき)
# 0.0.0.0 では待ち受けない。Tailscaleのアドレスがまだ無い(起動直後)ときは、失敗して終わる(systemdが10秒後にやり直す)。
set -eu
ROOT="${ROOT:-/opt/shiori-server}"
PORT="${PORT:-8090}"
if [ -z "${BIND:-}" ]; then
  IP="$(tailscale ip -4 2>/dev/null | head -n 1)"
  if [ -z "$IP" ]; then
    echo "Tailscaleのアドレスを取れない(tailscaledは起動していますか)" >&2
    exit 1
  fi
  BIND="$IP:$PORT"
fi
CONFIG="$ROOT/server.json"
if [ ! -f "$CONFIG" ]; then
  echo "設定が無い: $CONFIG(scripts/server/linux/server.example.json を写して直す)" >&2
  exit 1
fi
exec "$ROOT/bin/shiori-server" serve --config "$CONFIG" --bind "$BIND"
