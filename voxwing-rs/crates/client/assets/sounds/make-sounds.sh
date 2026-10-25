#!/usr/bin/env bash
# make-sounds.sh SCIFI_DIR UI_DIR MUSIC_FLAC JET_OGG AFTERBURNER_OGG OUT_DIR ENCODER
# Turns the Kenney CC0 packs, the two Freesound recordings and the music track into the game's
# sound files.
set -euo pipefail
S="$1"; U="$2"; M="$3"; JET="$4"; BURN="$5"; OUT="$6"; ENC="$7"
TMP="$(mktemp -d)"
mkdir -p "$OUT"
rm -f "$OUT"/*.ogg

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

# peak-normalise a wav to -1 dBFS, then encode it
finish() { # in.wav out-name quality
  local peak gain
  peak=$(ffmpeg -hide_banner -i "$1" -af volumedetect -f null - 2>&1 | awk '/max_volume/ {print $5}')
  gain=$(awk -v p="$peak" 'BEGIN { printf "%.2f", -1.0 - p }')
  ff -i "$1" -af "volume=${gain}dB" -c:a pcm_s16le "$TMP/n.wav"
  "$ENC" "$TMP/n.wav" "$OUT/$2.ogg" "${3:-0.6}"
}

# one sound: name, filter graph (or "-" for none), source files...
one() {
  local name="$1" filt="$2"; shift 2
  local args=()
  for f in "$@"; do args+=(-i "$f"); done
  if [ "$#" -gt 1 ]; then
    local mixin=""
    for i in $(seq 0 $(($# - 1))); do mixin+="[$i:a]"; done
    filt="${mixin}amix=inputs=$#:normalize=0${filt:+,$filt}"
    ff "${args[@]}" -filter_complex "$filt" -ar 44100 -c:a pcm_s16le "$TMP/a.wav"
  elif [ "$filt" = "-" ]; then
    ff "${args[@]}" -ar 44100 -c:a pcm_s16le "$TMP/a.wav"
  else
    ff "${args[@]}" -af "$filt" -ar 44100 -c:a pcm_s16le "$TMP/a.wav"
  fi
  finish "$TMP/a.wav" "$name"
}

# a short, dark tail: space has no air, but a hit with no decay at all sounds clipped
TAIL="aecho=0.8:0.55:70|140|230:0.32|0.2|0.1"

# weapons
for i in 0 1 2 3 4; do one "laser_$i" - "$S/laserSmall_00$i.ogg"; done
for i in 0 1 2 3 4; do one "plasma_$i" "$TAIL" "$S/laserLarge_00$i.ogg"; done
# rail: the retro zap pitched down a fifth, with a tail
for i in 0 1 2; do one "rail_$i" "asetrate=44100*0.7,aresample=44100,$TAIL" "$S/laserRetro_00$i.ogg"; done
# cannon: the crunch of a small explosion, pitched up and cut short to a punchy shot
for i in 0 1 2; do one "cannon_$i" "asetrate=44100*1.45,aresample=44100,atrim=0:0.32,afade=t=out:st=0.16:d=0.16" "$S/explosionCrunch_00$i.ogg"; done
# flak: low thump under a crunch
one flak_0 "atrim=0:0.9,afade=t=out:st=0.5:d=0.4,$TAIL" "$S/lowFrequency_explosion_001.ogg" "$S/explosionCrunch_000.ogg"
one flak_1 "atrim=0:0.9,afade=t=out:st=0.5:d=0.4,$TAIL" "$S/lowFrequency_explosion_001.ogg" "$S/explosionCrunch_002.ogg"
# missile launch: the first moment of a thruster burn
for i in 0 1; do one "missile_$i" "atrim=0:0.9,afade=t=in:d=0.03,afade=t=out:st=0.45:d=0.45" "$S/thrusterFire_00$i.ogg"; done

# hits and destruction
for i in 0 1 2 3 4; do one "hit_$i" - "$S/impactMetal_00$i.ogg"; done
for i in 0 1 2; do one "shield_$i" "atrim=0:0.6,afade=t=out:st=0.3:d=0.3" "$S/forceField_00$i.ogg"; done
for i in 1 2 3; do one "pop_$i" "atrim=0:0.7,afade=t=out:st=0.35:d=0.35" "$S/explosionCrunch_00$i.ogg"; done
one boom_0 "$TAIL" "$S/lowFrequency_explosion_000.ogg" "$S/explosionCrunch_004.ogg"
one boom_1 "$TAIL" "$S/lowFrequency_explosion_000.ogg" "$S/explosionCrunch_003.ogg"
for i in 1 3; do one "crash_$i" "asetrate=44100*0.72,aresample=44100" "$S/impactMetal_00$i.ogg"; done

# hit confirms, heard only by the shooter: a metal knock pitched up and cut short with a click on
# its edge (hull), a short glassy ting (shield), a confirm tone over a low thump (kill)
ff -i "$S/impactMetal_002.ogg" -i "$U/click_001.ogg" -filter_complex "[0:a]aformat=sample_fmts=fltp:channel_layouts=mono,asetrate=44100*1.5,aresample=44100,atrim=0:0.18,afade=t=out:st=0.08:d=0.1[a];[1:a]aformat=sample_fmts=fltp:channel_layouts=mono,aresample=44100,volume=0.7[b];[a][b]amix=inputs=2:normalize=0" -c:a pcm_s16le "$TMP/a.wav"
finish "$TMP/a.wav" confirm_hull
one confirm_shield "aformat=channel_layouts=mono,atrim=0:0.2,afade=t=out:st=0.1:d=0.1" "$U/glass_003.ogg"
ff -i "$U/confirmation_002.ogg" -i "$S/lowFrequency_explosion_001.ogg" -filter_complex "[0:a]aformat=sample_fmts=fltp:channel_layouts=mono,aresample=44100[a];[1:a]aformat=sample_fmts=fltp:channel_layouts=mono,aresample=44100,atrim=0:0.4,afade=t=out:st=0.15:d=0.25,volume=0.5[b];[a][b]amix=inputs=2:normalize=0" -c:a pcm_s16le "$TMP/a.wav"
finish "$TMP/a.wav" confirm_kill

# interface and hangar
one lock - "$U/confirmation_001.ogg"
one warn - "$U/error_006.ogg"
one deny - "$U/error_008.ogg"
for i in 1 2 3 4; do one "place_$i" - "$U/drop_00$i.ogg"; done
one erase - "$U/close_001.ogg"
one wave - "$U/confirmation_004.ogg"
one hover - "$U/tick_001.ogg"
one click - "$U/click_001.ogg"

# a seamless loop from the steady part of a recording: its last stretch crossfades into its
# first (in float with headroom, so the overlap can't wrap around)
loop() { # name, source, start, end, crossfade seconds
  ff -i "$2" -filter_complex "[0:a]atrim=start=${3}:end=${4},asetpts=PTS-STARTPTS,aresample=44100,aformat=sample_fmts=fltp,volume=0.6,asplit=2[a][b];[a]atrim=start=${5},asetpts=PTS-STARTPTS[A];[b]atrim=end=${5},asetpts=PTS-STARTPTS[B];[A][B]acrossfade=d=${5}:c1=qsin:c2=qsin" -c:a pcm_s16le "$TMP/l.wav"
  finish "$TMP/l.wav" "$1"
}

# jet engine (a static test run) and afterburner: steady loops, plus the afterburner's ignition
loop engine "$JET" 0.5 9.5 1.0
loop burner "$BURN" 2.0 5.2 0.6
one ignite "atrim=0:1.3,afade=t=out:st=0.7:d=0.6" "$BURN"

# music: the track's own seamless loop, 48 kHz stereo
ff -i "$M" -c:a pcm_s16le "$TMP/m.wav"
"$ENC" "$TMP/m.wav" "$OUT/music.ogg" 0.45

rm -rf "$TMP"
ls "$OUT" | wc -l
du -sh "$OUT"
