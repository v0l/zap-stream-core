#!/bin/bash

ffmpeg 	\
	-re -f lavfi -i testsrc -g 60 -r 30 -pix_fmt yuv420p -s 1280x720 -c:v h264 -b:v 2000k -c:a aac -b:a 192k \
	-f mpegts srt://localhost:3333
