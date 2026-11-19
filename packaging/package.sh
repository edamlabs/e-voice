#!/usr/bin/env bash
# Stages the native install and builds both artifacts into OUT: evoice_<v>_amd64.deb (dpkg-deb) and
# evoice-<v>-linux-x86_64.tar.gz, same content. The glibc floor is read from the staged binaries and libraries.
# Usage: packaging/package.sh <version> <dist> <out> <bin>...
set -euo pipefail
umask 022

VERSION="${1:?version}"
DIST="${2:?dist}"
OUT="${3:?out}"
shift 3
BINS=("$@")
(( ${#BINS[@]} )) || { echo "package: no binaries" >&2; exit 1; }

DEB="${OUT}/evoice_${VERSION}_amd64"
TAR="${OUT}/evoice-${VERSION}-linux-x86_64"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"
rm -rf "${DEB}" "${TAR}" "${DEB}.deb" "${TAR}.tar.gz"

# Fills a template's @KEY@ placeholders: render <template> <target> KEY=value...
render() {
    local template="$1" target="$2" script=()
    shift 2
    for pair in "$@"; do script+=(-e "s|@${pair%%=*}@|${pair#*=}|g"); done
    mkdir -p "${target%/*}"
    sed "${script[@]}" "${template}" > "${target}"
    chmod 644 "${target}"
}

# The same content under both layouts: stage <bin> <lib> <share> <doc> <units> <prefix the units run from>
stage() {
    local bin="$1" lib="$2" share="$3" doc="$4" units="$5" prefix="$6"
    for name in "${BINS[@]}"; do install -D -m 755 "${DIST}/bin/${name}" "${bin}/${name}"; done
    for so in "${DIST}"/lib/*.so*; do install -D -m 644 "${so}" "${lib}/${so##*/}"; done
    for service in stt tts; do install -D -m 644 "evoice/${service}/models.toml" "${share}/${service}/models.toml"; done
    install -D -m 644 packaging/evoice.toml "${share}/evoice.toml"
    install -D -m 644 packaging/copyright "${doc}/copyright"
    install -D -m 644 evoice.example.toml "${doc}/evoice.example.toml"
    for unit in packaging/systemd/*.service; do
        render "${unit}" "${units}/${unit##*/}" "BIN=${prefix}/bin" "SHARE=${prefix}/share/evoice"
    done
}

##### DEB #####
stage "${DEB}/usr/lib/evoice/bin" "${DEB}/usr/lib/evoice/lib" "${DEB}/usr/share/evoice" \
    "${DEB}/usr/share/doc/evoice" "${DEB}/usr/lib/systemd/user" /usr
mkdir -p "${DEB}/usr/bin"
for name in "${BINS[@]}"; do ln -s "../lib/evoice/bin/${name}" "${DEB}/usr/bin/${name}"; done
GLIBC="$(objdump -T "${DEB}"/usr/lib/evoice/bin/* "${DEB}"/usr/lib/evoice/lib/*.so* \
    | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -Vu | tail -1)"
[[ -n "${GLIBC}" ]] || { echo "package: no GLIBC_ version required, objdump found nothing" >&2; exit 1; }
render packaging/control "${DEB}/DEBIAN/control" "VERSION=${VERSION}" "GLIBC=${GLIBC}" \
    "SIZE=$(du -sk --apparent-size "${DEB}/usr" | cut -f1)"
dpkg-deb --build --root-owner-group "${DEB}" "${DEB}.deb" >/dev/null

##### TARBALL #####
stage "${TAR}/bin" "${TAR}/lib" "${TAR}/share/evoice" "${TAR}/share/doc/evoice" "${TAR}/systemd" \
    "%h/.local/opt/evoice"
render packaging/README "${TAR}/README" "VERSION=${VERSION}" "GLIBC=${GLIBC}"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@${SOURCE_DATE_EPOCH}" -C "${OUT}" \
    -cf - "${TAR##*/}" | gzip -n > "${TAR}.tar.gz"

rm -rf "${DEB}" "${TAR}"
printf 'glibc >= %s\n%s\n%s\n' "${GLIBC}" "${DEB}.deb" "${TAR}.tar.gz"
