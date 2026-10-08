#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
game_dir=""
install_mod=false
while (($#)); do
  case "$1" in
    --game-path) game_dir="${2:?--game-path requires a directory}"; shift 2 ;;
    --install-mod) install_mod=true; shift ;;
    *) echo "Usage: bash scripts/setup.sh [--game-path DIRECTORY] [--install-mod]" >&2; exit 2 ;;
  esac
done
command -v cargo >/dev/null || { echo "Install current stable Rust from https://rustup.rs/" >&2; exit 1; }
command -v dotnet >/dev/null || { echo "Install the .NET 8 SDK from https://dotnet.microsoft.com/download/dotnet/8.0" >&2; exit 1; }
if [[ -z "$game_dir" ]]; then
  for candidate in "$HOME/.local/share/Steam/steamapps/common/Stardew Valley" "$HOME/.steam/steam/steamapps/common/Stardew Valley"; do
    if [[ -f "$candidate/Stardew Valley.dll" ]]; then game_dir="$candidate"; break; fi
  done
fi
cd -- "$project_dir"
cargo build --release --locked
if [[ -n "$game_dir" ]]; then
  [[ -f "$game_dir/StardewModdingAPI.dll" ]] || { echo "Install SMAPI into $game_dir using https://smapi.io/ first." >&2; exit 1; }
  dotnet build mod/Observer/Observer.csproj -c Release "-p:GamePath=$game_dir"
  if $install_mod; then
    destination="$game_dir/Mods/SvOptimizer.Observer"
    if [[ -e "$destination" ]]; then
      backup="$destination.backup.$(date +%Y%m%dT%H%M%S)"
      [[ ! -e "$backup" ]] || { echo "Backup already exists: $backup" >&2; exit 1; }
      mv -- "$destination" "$backup"
    fi
    mkdir -p -- "$destination"
    cp -a -- mod/Observer/bin/Release/net6.0/. "$destination/"
    if [[ -n "${backup:-}" && -f "$backup/config.json" ]]; then cp -- "$backup/config.json" "$destination/config.json"; fi
    echo "Observer installed in $destination"
  fi
elif $install_mod; then
  echo "Game not detected; pass --game-path DIRECTORY to install the observer." >&2; exit 1
else
  echo "Game not detected. Rust built; pass --game-path DIRECTORY to build the observer."
fi
target/release/sv-optimizer doctor --hardware-only
echo "Install/run Ollama and explicitly download the recommended model. Then run target/release/sv-optimizer chat."
