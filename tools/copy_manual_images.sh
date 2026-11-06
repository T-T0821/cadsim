#!/usr/bin/env bash
# README 用に撮った画面写真 (docs/screenshots/) のうち、操作マニュアル (docs/src/) でも使うものを docs/src/images/ に写す。
# 画面写真を撮り直したあとに流す。
set -euo pipefail
cd "$(dirname "$0")/.."
src=docs/screenshots
dst=docs/src/images
while read -r from to; do
    [[ -z "$from" || "$from" == \#* ]] && continue
    cp "$src/$from" "$dst/$to"
done <<'LIST'
ai_help/window.png              ai_help.png
compose/placed.png              compose_placed.png
review/peers.png                review_peers.png
render_quality/01_high.png      render_quality_high.png
factory_hall/01_demo_line.png   factory_hall.png
sound/alarm.png                 sound_alarm.png
effects/running.png             effects_running.png
urdf/ur5e.png                   urdf_ur5e.png
real_robot/playing.png          real_robot_playing.png
real_robot/grip.png             real_robot_grip.png
factory/lines.png               factory_lines.png
factory/result.png              factory_result.png
plan/element.png                plan_element.png
plan/result.png                 plan_result.png
28_scenarios.png                scenarios.png
heatmap/floor.png               heatmap_floor.png
heatmap/panel.png               heatmap_panel.png
menus/video_camera_path.png     video_camera_path.png
walk/narrow_aisle.png           walk_narrow_aisle.png
part_role/cylinder.png          part_role_cylinder.png
LIST
