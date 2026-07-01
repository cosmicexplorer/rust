#!/bin/zsh

set -euxo pipefail

declare -r arg_path="$1"
declare -r hash="$2"

pushd "$arg_path"
git checkout -
git checkout "$hash"
popd

git submodule status --cached -- \
    src/doc/nomicon src/doc/reference src/doc/rust-by-example src/llvm-project src/tools/cargo \
  | sed -re 's#^\+([^[:space:]]+)[[:space:]]+([^[:space:]]+)[[:space:]].*$#\2 \1#' \
  | (set -x; while read name checksum; do
               (pushd "$name" && git checkout - && git checkout "$checksum");
             done)
