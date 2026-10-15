#!/bin/sh
# Checks that each argument is an aarch64 Linux ELF that runs on Amazon Linux 2023 (glibc 2.34).
# Runs inside the build container (needs file and objdump).
set -eu

CEILING=${GLIBC_CEILING:-2.34}
status=0
for path in "$@"; do
  name=$(basename "$path")
  if ! file -b "$path" | grep -q 'ELF 64-bit LSB.*ARM aarch64'; then
    echo "$name: not an aarch64 ELF executable" >&2
    status=1
    continue
  fi
  need=$(objdump -T "$path" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
  top=$(printf '%s\n%s\n' "$need" "$CEILING" | sort -V | tail -n 1)
  if [ "$top" != "$CEILING" ]; then
    echo "$name: needs glibc $need, newer than $CEILING on Amazon Linux 2023" >&2
    status=1
    continue
  fi
  echo "$name: aarch64 ELF, highest glibc symbol $need"
done
exit "$status"
