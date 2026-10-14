#!/bin/sh
# Packs the release binaries into dist/lunima-team-<version>-<rid>.tar.gz (used by CI).
#   packaging/linux/make-tarball.sh <version> <rid>     (run from the repository root after
#   cargo build --release -p team-agent -p lunima-po-center)
set -eu
version=$1
rid=$2
here=$(cd "$(dirname "$0")" && pwd)
name="lunima-team-$version-$rid"
stage="dist/$name"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/systemd/user" "$stage/share/applications" "$stage/share/icons"
install -m 755 target/release/lunima-team-agent "$stage/bin/"
install -m 755 target/release/lunima-po "$stage/bin/"
install -m 644 "$here/lunima-team-agent.service" "$stage/share/systemd/user/"
install -m 644 "$here/lunima-team-control-center.desktop" "$stage/share/applications/"
install -m 644 "$here/lunima-team-control-center.png" "$stage/share/icons/"
install -m 755 "$here/install.sh" "$stage/"
cp README.md "$stage/"
tar -C dist -czf "dist/$name.tar.gz" "$name"
rm -rf "$stage"
echo "dist/$name.tar.gz"
