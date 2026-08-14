#!/bin/sh
set -eu
grep -q '320' index.html
grep -q 'colors = \["#dc2626", "#15803d", "#dc2626", "#15803d"\]' index.html
grep -q 'translateX(160px)' index.html
