#!/usr/bin/env bash
# Fetches the parse corpora into ./corpus (ignored by git): a pinned phpstorm-stubs commit and the
# test directories of a pinned php-src tag. Run it from anywhere; it works relative to this script.
set -euo pipefail

STUBS_REPO="https://github.com/JetBrains/phpstorm-stubs"
STUBS_COMMIT="5f84a9c87737addd09ad6cc4ad9bd4afb6f794a1"
PHP_SRC_REPO="https://github.com/php/php-src"
PHP_SRC_TAG="php-8.6.0"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$root/corpus"
mkdir -p "$corpus"

if [ ! -d "$corpus/phpstorm-stubs/.git" ]; then
    git init -q "$corpus/phpstorm-stubs"
    git -C "$corpus/phpstorm-stubs" remote add origin "$STUBS_REPO"
fi
git -C "$corpus/phpstorm-stubs" fetch -q --depth 1 origin "$STUBS_COMMIT"
git -C "$corpus/phpstorm-stubs" checkout -q --detach FETCH_HEAD
echo "phpstorm-stubs at $STUBS_COMMIT"

if [ ! -d "$corpus/php-src/.git" ]; then
    git clone -q --depth 1 --branch "$PHP_SRC_TAG" --filter=blob:none --sparse "$PHP_SRC_REPO" "$corpus/php-src"
elif [ "$(git -C "$corpus/php-src" describe --tags --exact-match 2>/dev/null)" != "$PHP_SRC_TAG" ]; then
    git -C "$corpus/php-src" fetch -q --depth 1 origin tag "$PHP_SRC_TAG"
    git -C "$corpus/php-src" checkout -q --detach FETCH_HEAD
fi
git -C "$corpus/php-src" sparse-checkout set Zend/tests tests
echo "php-src at $PHP_SRC_TAG"
