#!/bin/sh
# Builds play/index.html from voxwing.html: adds the HTML skeleton, points three.js at the
# local copy in play/node_modules, and loads Google Fonts without blocking the page.
cd "$(dirname "$0")" || exit 1
{
  printf '<!doctype html>\n<html lang="zh-CN"><head><meta charset="utf-8">\n'
  printf '<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">\n'
  printf '</head><body>\n'
  sed \
    -e 's#https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.module.js#./node_modules/three/build/three.module.js#' \
    -e 's#https://cdn.jsdelivr.net/npm/three@0.160.0/examples/jsm/#./node_modules/three/examples/jsm/#' \
    -e 's#<link rel="stylesheet" href="\(https://fonts.googleapis.com/[^"]*\)">#<link rel="stylesheet" href="\1" media="print" onload="this.media='"'"'all'"'"'">#' \
    voxwing.html
  printf '\n</body></html>\n'
} > play/index.html
grep -c "node_modules/three" play/index.html
