#!/bin/sh
cd /usr/share/vkquake2 || exit 1
exec ./quake2 +set vid_ref vk +set vid_fullscreen 1 +set vk_validation 0 +set vk_point_particles 0 +set nocdaudio 1 +set no_music 1 "$@"
