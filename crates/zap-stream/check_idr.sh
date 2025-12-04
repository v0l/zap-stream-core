#!/bin/bash
# Check if all fMP4 segments start with I-frames

cd out
find . -name "*.m4s" | sort -V | while read seg; do
  dir=$(dirname "$seg")
  init="$dir/init.mp4"

  if [ ! -f "$init" ]; then
    echo "$seg: NO INIT"
    continue
  fi

  cat "$init" "$seg" > /tmp/check_idr.mp4 2>/dev/null

  # Get all frame types in the segment
  frames=$(ffprobe -show_frames -select_streams v:0 /tmp/check_idr.mp4 2>&1 | grep "pict_type=" | cut -d= -f2 | tr '\n' ' ')

  if [ -z "$frames" ]; then
    echo "$seg: NO VIDEO FRAMES"
  else
    first_frame=$(echo "$frames" | awk '{print $1}')
    if [ "$first_frame" != "I" ]; then
      echo "$seg: FAIL (first=$first_frame) [$frames]"
    else
      echo "$seg: OK [$frames]"
    fi
  fi
done

rm -f /tmp/check_idr.mp4