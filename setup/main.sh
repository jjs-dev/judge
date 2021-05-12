#!/usr/bin/env bash
set -euxo pipefail

echo "Setting up problems"
cd /etc/problems
for i in *; do
    mkdir -p /var/problems/$i
    pps-cli compile --pkg /etc/problems/$i --out /var/problems/$i --force
done

echo "Setting up toolchains"
cd /etc/toolchains
for i in *; do
    mkdir -p /var/toolchains/$i
    cp $i/manifest.yaml /var/toolchains/$i/manifest.yaml
    echo "ghcr.io/jjs-dev/toolchain-$i:latest" > /var/toolchains/$i/image.txt
done