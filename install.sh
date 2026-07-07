#!/usr/bin/env sh
set -eu

repo="${DICTA_REPO:-kingsword09/dicta}"
version="${DICTA_VERSION:-latest}"
install_dir="${DICTA_INSTALL_DIR:-$HOME/.local/bin}"
archive_override="${DICTA_ARCHIVE:-}"
base_url_override="${DICTA_BASE_URL:-}"

usage() {
  cat <<'USAGE'
dicta installer

Usage:
  curl -fsSL https://raw.githubusercontent.com/kingsword09/dicta/main/install.sh | sh
  ./install.sh --uninstall

Environment:
  DICTA_REPO         GitHub repository, default: kingsword09/dicta
  DICTA_VERSION      Release version or tag, default: latest
  DICTA_INSTALL_DIR  Install directory, default: $HOME/.local/bin
  DICTA_ARCHIVE      Local archive path to install from
  DICTA_BASE_URL     Base URL or file:// directory containing release archives
USAGE
}

fail() {
  echo "dicta install: $*" >&2
  exit 1
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  --uninstall)
    case "$(uname -s)" in
      MINGW*|MSYS*|CYGWIN*) exe_name="dicta.exe" ;;
      *) exe_name="dicta" ;;
    esac
    case "$exe_name" in
      *.exe) tray_name="dicta-tray.exe"; adapter_name="dicta-adapter-apple-speech.exe" ;;
      *) tray_name="dicta-tray"; adapter_name="dicta-adapter-apple-speech" ;;
    esac
    target="$install_dir/$exe_name"
    tray_target="$install_dir/$tray_name"
    adapter_target="$install_dir/$adapter_name"
    if [ -e "$target" ]; then
      rm -f "$target"
      echo "dicta install: removed $target"
    else
      echo "dicta install: $target is not installed"
    fi
    if [ -e "$adapter_target" ]; then
      rm -f "$adapter_target"
      echo "dicta install: removed $adapter_target"
    fi
    if [ -e "$tray_target" ]; then
      rm -f "$tray_target"
      echo "dicta install: removed $tray_target"
    fi
    exit 0
    ;;
  "")
    ;;
  *)
    fail "unknown argument: $1"
    ;;
esac

need() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

need uname
need mktemp

case "$(uname -s)" in
  Darwin) os="darwin" ;;
  Linux) os="linux" ;;
  MINGW*|MSYS*|CYGWIN*) os="windows" ;;
  *) fail "unsupported OS: $(uname -s)" ;;
esac

machine="$(uname -m)"
if [ "$os" = "windows" ]; then
  windows_arch="${PROCESSOR_ARCHITEW6432:-${PROCESSOR_ARCHITECTURE:-}}"
  case "$windows_arch" in
    ARM64|arm64|aarch64|AARCH64) machine="arm64" ;;
  esac
fi

case "$machine" in
  arm64|aarch64|ARM64)
    arch="arm64"
    ;;
  x86_64|amd64|AMD64)
    arch="x86_64"
    ;;
  *) fail "unsupported architecture: $machine" ;;
esac

if [ "$os" = "windows" ]; then
  archive_ext="zip"
  exe_name="dicta.exe"
  tray_name="dicta-tray.exe"
  adapter_name="dicta-adapter-apple-speech.exe"
else
  archive_ext="tar.gz"
  exe_name="dicta"
  tray_name="dicta-tray"
  adapter_name="dicta-adapter-apple-speech"
fi

if [ "$os" = "darwin" ] && [ "$arch" != "arm64" ]; then
  fail "no prebuilt archive is published for darwin_$arch"
fi

darwin_major_version() {
  if [ "$os" != "darwin" ]; then
    echo 0
    return
  fi
  sw_vers -productVersion 2>/dev/null | awk -F. '{print $1}'
}

supports_apple_adapter() {
  [ "$os" = "darwin" ] || return 1
  [ "$(darwin_major_version)" -ge 26 ] 2>/dev/null
}

need_url_tool=""
if command -v curl >/dev/null 2>&1; then
  need_url_tool="curl"
elif command -v wget >/dev/null 2>&1; then
  need_url_tool="wget"
else
  fail "required command not found: curl or wget"
fi

if [ "$archive_ext" = "zip" ]; then
  need unzip
else
  need tar
fi

tmp="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

fetch() {
  url="$1"
  output="$2"
  if [ "$need_url_tool" = "curl" ]; then
    curl --fail --location --silent "$url" --output "$output"
  else
    wget -q "$url" -O "$output"
  fi
}

if [ "$version" = "latest" ]; then
  tag=""
  base_url="${base_url_override:-https://github.com/$repo/releases/latest/download}"
  display_version="latest"
else
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
  base_url="${base_url_override:-https://github.com/$repo/releases/download/$tag}"
  display_version="$tag"
fi

suffix="${os}_${arch}"
archive="dicta_${suffix}.${archive_ext}"
url="$base_url/$archive"

archive_path="$tmp/$archive"
download_archive() {
  download_url="$1"
  download_path="$2"
  case "$download_url" in
    file://*)
      cp "${download_url#file://}" "$download_path"
      return
      ;;
  esac
  if [ "$need_url_tool" = "curl" ]; then
    curl --fail --location --progress-bar "$download_url" --output "$download_path"
  else
    wget -q --show-progress "$download_url" -O "$download_path"
  fi
}

if [ -n "$archive_override" ]; then
  [ -f "$archive_override" ] || fail "archive does not exist: $archive_override"
  archive_path="$archive_override"
  echo "dicta install: installing from local archive $archive_path"
elif [ -f "$url" ]; then
  archive_path="$url"
  echo "dicta install: installing from local archive $archive_path"
else
  echo "dicta install: downloading $repo $display_version for $suffix"
  echo "dicta install: $url"
  if ! download_archive "$url" "$archive_path"; then
    if [ "$version" = "latest" ]; then
      if [ "$need_url_tool" = "curl" ]; then
        latest_url="$(curl --fail --location --silent --output /dev/null --write-out '%{url_effective}' "https://github.com/$repo/releases/latest")"
        tag="${latest_url##*/}"
        [ -n "$tag" ] || fail "download failed: $url"
        versioned_archive="dicta_${tag}_${suffix}.${archive_ext}"
        versioned_url="https://github.com/$repo/releases/download/$tag/$versioned_archive"
        echo "dicta install: retrying versioned asset name: $versioned_url"
        archive_path="$tmp/$versioned_archive"
        download_archive "$versioned_url" "$archive_path" || fail "download failed: $versioned_url"
      else
        fail "download failed: $url"
      fi
    else
      versioned_archive="dicta_${tag}_${suffix}.${archive_ext}"
      versioned_url="$base_url/$versioned_archive"
      echo "dicta install: retrying versioned asset name: $versioned_url"
      archive_path="$tmp/$versioned_archive"
      download_archive "$versioned_url" "$archive_path" || fail "download failed: $versioned_url"
    fi
  fi
fi

mkdir -p "$tmp/extract"
if [ "$archive_ext" = "zip" ]; then
  unzip -q "$archive_path" -d "$tmp/extract"
else
  tar -xzf "$archive_path" -C "$tmp/extract"
fi

bin_path="$(find "$tmp/extract" -type f -name "$exe_name" -perm -u+x | head -n 1)"
if [ -z "$bin_path" ]; then
  bin_path="$(find "$tmp/extract" -type f -name "$exe_name" | head -n 1)"
fi
[ -n "$bin_path" ] || fail "archive did not contain $exe_name"

mkdir -p "$install_dir"
target="$install_dir/$exe_name"
cp "$bin_path" "$target"
chmod 755 "$target"

adapter_path="$(find "$tmp/extract" -type f -name "$adapter_name" -perm -u+x | head -n 1)"
if [ -z "$adapter_path" ]; then
  adapter_path="$(find "$tmp/extract" -type f -name "$adapter_name" | head -n 1)"
fi
if [ -z "$adapter_path" ]; then
  adapter_path="$(find "$tmp/extract" -type f -name "$adapter_name" | head -n 1)"
fi
if [ -n "$adapter_path" ] && { [ "$os" != "darwin" ] || supports_apple_adapter; }; then
  adapter_target="$install_dir/$adapter_name"
  cp "$adapter_path" "$adapter_target"
  chmod 755 "$adapter_target"
elif [ -n "$adapter_path" ] && [ "$os" = "darwin" ]; then
  echo "dicta install: skipped $adapter_name (requires macOS 26+)"
fi

tray_path="$(find "$tmp/extract" -type f -name "$tray_name" -perm -u+x | head -n 1)"
if [ -z "$tray_path" ]; then
  tray_path="$(find "$tmp/extract" -type f -name "$tray_name" | head -n 1)"
fi
if [ -n "$tray_path" ]; then
  tray_target="$install_dir/$tray_name"
  cp "$tray_path" "$tray_target"
  chmod 755 "$tray_target"
fi

if [ "$os" = "darwin" ] && command -v xattr >/dev/null 2>&1; then
  xattr -cr "$target" >/dev/null 2>&1 || true
  if [ -n "${tray_target:-}" ]; then
    xattr -cr "$tray_target" >/dev/null 2>&1 || true
  fi
  if [ -n "${adapter_target:-}" ]; then
    xattr -cr "$adapter_target" >/dev/null 2>&1 || true
  fi
fi

if [ "$os" = "darwin" ] && command -v codesign >/dev/null 2>&1; then
  codesign --force --sign - "$target" >/dev/null 2>&1 || true
  if [ -n "${tray_target:-}" ]; then
    codesign --force --sign - "$tray_target" >/dev/null 2>&1 || true
  fi
fi

echo "dicta install: installed $target"
if [ -n "${adapter_target:-}" ]; then
  echo "dicta install: installed $adapter_target"
fi
if [ -n "${tray_target:-}" ]; then
  echo "dicta install: installed $tray_target"
fi
version_out="$tmp/version.out"
version_err="$tmp/version.err"
if ! "$target" --version >"$version_out" 2>"$version_err"; then
  echo "dicta install: warning: installed binary could not run on this system"
  if [ -s "$version_err" ]; then
    sed 's/^/  /' "$version_err" >&2
  fi
else
  cat "$version_out"
fi
if [ ":$PATH:" != *":$install_dir:"* ]; then
  echo "dicta install: add this to your shell profile:"
  echo "  export PATH=\"$install_dir:\$PATH\""
fi
