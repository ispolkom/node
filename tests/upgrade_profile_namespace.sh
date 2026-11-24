#!/usr/bin/env bash
# Run under unshare -Urnmp --fork from the repository root. Host system directories are read-only bind mounts;
# only the synthetic upgrade profile is writable in the private root.
set -euo pipefail
namespace_root=${1:?synthetic root required}
repo_root=$(pwd -P)
mount -t tmpfs -o size=32m tmpfs "$namespace_root/root"
for path in usr lib lib64 bin etc proc dev sys; do
    mkdir -p "$namespace_root/root/$path"
done
mkdir -p "$namespace_root/root$namespace_root" "$namespace_root/root$repo_root"
for path in usr lib lib64 bin etc; do
    mount --bind "/$path" "$namespace_root/root/$path"
    mount -o remount,bind,ro "$namespace_root/root/$path"
done
mount -t proc proc "$namespace_root/root/proc"
mkdir -p "$namespace_root/root/sys/class/dmi/id" "$namespace_root/root/sys/class/net"
for name in board_vendor board_name product_name sys_vendor; do
    cp "/sys/class/dmi/id/$name" "$namespace_root/root/sys/class/dmi/id/$name"
done
for nic in /sys/class/net/*; do
    if [[ -e "$nic/device" ]]; then
        mkdir -p "$namespace_root/root/sys/class/net/${nic##*/}/device"
        cp "$nic/address" "$namespace_root/root/sys/class/net/${nic##*/}/address"
    fi
done
mount -t tmpfs -o size=1m tmpfs "$namespace_root/root/dev"
mkdir -p "$namespace_root/root/dev/disk/by-id"
mount --bind /dev/disk/by-id "$namespace_root/root/dev/disk/by-id"
mount -o remount,bind,ro "$namespace_root/root/dev/disk/by-id"
touch "$namespace_root/root/dev/null" "$namespace_root/root/dev/urandom"
mount --bind /dev/null "$namespace_root/root/dev/null"
mount --bind /dev/urandom "$namespace_root/root/dev/urandom"
mount --bind "$repo_root" "$namespace_root/root$repo_root"
mount -o remount,bind,ro "$namespace_root/root$repo_root"
mount --bind "$namespace_root" "$namespace_root/root$namespace_root"
/usr/sbin/chroot "$namespace_root/root" /bin/bash "$repo_root/tests/upgrade_profile_probe.sh" \
    "$namespace_root" "$namespace_root/newpkg/usr/bin/yandi"
