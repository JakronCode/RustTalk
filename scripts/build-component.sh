#!/usr/bin/env bash
set -euo pipefail
component=${1-all}
action=${2-validate}
case "$component" in
    all) packages=(--workspace) ;;
    client|server) packages=(-p "rusttalk-$component" -p rusttalk-shared) ;;
    *) echo "Unsupported component: $component (expected all, client or server)" >&2; exit 1 ;;
esac
case "$action" in
    validate) ;;
    test) cargo test --locked "${packages[@]}" -- --test-threads=1 ;;
    build) cargo build --locked --release "${packages[@]}" ;;
    *) echo "Unsupported build action: $action" >&2; exit 1 ;;
esac
