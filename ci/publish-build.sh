set -euxo pipefail

docker build -t judge --build-arg 'RELEASE=--release' .
