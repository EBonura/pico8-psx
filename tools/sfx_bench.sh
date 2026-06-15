#!/usr/bin/env bash
# SFX similarity testbench runner: build the soundtest disc for a game, capture
# the PSX SPU output, and score every SFX against its PICO-8 reference recording.
#
#   tools/sfx_bench.sh <celeste|celeste2> [--dump DIR]
#
# Env: SECS (capture seconds, default 135).
set -euo pipefail
GAME="${1:?usage: sfx_bench.sh <celeste|celeste2> [sfx_bench.py args]}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EXE="games/$GAME/target/mipsel-sony-psx/release/soundtest.exe"
WAV="/tmp/${GAME}_soundtest.wav"
SECS="${SECS:-135}"

echo "== build $GAME soundtest =="
( cd "games/$GAME" && cargo build --release --bin soundtest >/dev/null 2>&1 )
echo "== pack disc =="
( cd third_party/PSoXide/tools/mkisopsx && cargo run --release -- \
    --exe "$ROOT/$EXE" --out "$ROOT/dist/${GAME}_soundtest.bin" --volume PICO8PSX >/dev/null 2>&1 )
echo "== capture ${SECS}s SPU -> $WAV =="
cargo run -q --manifest-path tools/psx-audio-capture/Cargo.toml --bin psx-audio-capture -- \
    --disc "dist/${GAME}_soundtest.cue" --out "$WAV" --seconds "$SECS" 2>/dev/null
echo "== score =="
python3 tools/sfx_bench.py "$WAV" "$GAME" "${@:2}"
