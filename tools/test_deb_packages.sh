#!/usr/bin/env bash
# Install and smoke-test the two locally built packages inside a disposable
# Debian or Ubuntu test environment. No host data or node keys are used.
set -euo pipefail

PKG_DIR=${1:?usage: test_deb_packages.sh PACKAGE_DIR}
NODE_DEB=$(find "$PKG_DIR" -maxdepth 1 -name 'yandi-node_*.deb' -print -quit)
DESKTOP_DEB=$(find "$PKG_DIR" -maxdepth 1 -name 'yandi-desktop_*.deb' -print -quit)
[ -n "$NODE_DEB" ] && [ -n "$DESKTOP_DEB" ] || {
  echo "both yandi-node and yandi-desktop packages are required" >&2
  exit 2
}

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends systemd systemd-sysv xvfb xauth dbus-x11
apt-get install -y "/packages/$(basename "$NODE_DEB")" "/packages/$(basename "$DESKTOP_DEB")"

dpkg-query -W -f='${Status}\n' yandi-node | grep -qx 'install ok installed'
dpkg-query -W -f='${Status}\n' yandi-desktop | grep -qx 'install ok installed'
test "$(dpkg-query -W -f='${Architecture}' yandi-node)" = amd64
test "$(dpkg-query -W -f='${Architecture}' yandi-desktop)" = amd64

/usr/bin/yandi --version
systemd-analyze verify /usr/lib/systemd/system/yandi-node@.service
test -x /usr/bin/yandi-desktop
! ldd /opt/yandi/yandi_desktop 2>&1 | grep -q 'not found'

useradd --create-home --shell /bin/bash yandi-ci
runuser -u yandi-ci -- env HOME=/home/yandi-ci /usr/bin/yandi setup --help >/dev/null
timeout 8s runuser -u yandi-ci -- env HOME=/home/yandi-ci \
  xvfb-run -a -s '-screen 0 1280x800x24' /usr/bin/yandi-desktop >/tmp/yandi-desktop.log 2>&1 || rc=$?
if [ "${rc:-0}" -ne 0 ] && [ "${rc:-0}" -ne 124 ]; then
  cat /tmp/yandi-desktop.log >&2
  exit "$rc"
fi

apt-get purge -y yandi-desktop yandi-node
! dpkg-query -W yandi-desktop >/dev/null 2>&1
! dpkg-query -W yandi-node >/dev/null 2>&1
echo "deb package install, smoke test, and removal: PASS"
