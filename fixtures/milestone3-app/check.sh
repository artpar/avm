#!/usr/bin/env bash
set -euo pipefail

grep -Fq 'Count: 0' index.html
grep -Fq 'count += 1' index.html
