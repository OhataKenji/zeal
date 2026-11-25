#!/bin/sh
set -eu

: "${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the absolute path of tla2tools.jar}"
exec python3 "$(dirname "$0")/check_models.py"
