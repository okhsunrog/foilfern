#!/usr/bin/env bash
# Prepares a fresh machine (e.g. a Claude Code cloud environment, CI) for
# working on librepcb-rs:
#
# 1. system packages for building and for running the upstream CLI headless
#    (Debian/Ubuntu only; skipped elsewhere),
# 2. a Rust toolchain if `cargo` is missing,
# 3. the upstream LibrePCB checkout used by the tests (pinned commit, with the
#    submodules the tests need) at `LIBREPCB_UPSTREAM_DIR`,
# 4. the official upstream `librepcb-cli` (the comparison oracle), wrapped in
#    `xvfb-run` because the release build only ships the xcb Qt platform
#    plugin,
# 5. the FreeRouting autorouter (Java >= 25, jar in the tools directory) with
#    a headless `freerouting` launcher, used by `librepcb-editor`'s
#    `FreeroutingRouter` and its round trip tests.
#
# Afterwards `cargo test --workspace` runs everything, including the
# comparisons against the upstream CLI.
#
# The script is idempotent. Environment overrides:
#   LIBREPCB_UPSTREAM_DIR  upstream checkout (default: ../LibrePCB next to
#                          this repository, as in .cargo/config.toml)
#   LIBREPCB_TOOLS_DIR     where the CLI release is unpacked
#                          (default: ~/.local/share/librepcb-rs)
#   LIBREPCB_BIN_DIR       where the `librepcb-cli` and `freerouting`
#                          wrappers are installed (default: /usr/local/bin
#                          if writable, else ~/.local/bin)
#   LIBREPCB_FREEROUTING_JAVA  Java >= 25 for FreeRouting (default: the
#                          first one found in /usr/lib/jvm or the PATH)

set -euo pipefail

UPSTREAM_REPO="https://github.com/LibrePCB/LibrePCB.git"
UPSTREAM_REV="7f1b548abb61bd59b2b9c1166eca3e6557480c17" # 2.1.1 + 37 commits
UPSTREAM_SUBMODULES=(tests/data share/librepcb/fontobene i18n libs/fontobene-qt)
CLI_VERSION="2.1.1"
CLI_BASE_URL="https://download.librepcb.org/releases/${CLI_VERSION}"
CLI_ARCHIVE="librepcb-${CLI_VERSION}-linux-x86_64.tar.gz"
FREEROUTING_VERSION="2.4.1"
FREEROUTING_URL="https://github.com/freerouting/freerouting/releases/download/v${FREEROUTING_VERSION}/freerouting-${FREEROUTING_VERSION}.jar"
FREEROUTING_SHA256="251101c3eeac22d7e7dfcf6796603279e5d1000283eb82d8f093780f7afc6aa9"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPSTREAM_DIR="${LIBREPCB_UPSTREAM_DIR:-${ROOT}/../LibrePCB}"
TOOLS_DIR="${LIBREPCB_TOOLS_DIR:-${HOME}/.local/share/librepcb-rs}"
if [[ -n "${LIBREPCB_BIN_DIR:-}" ]]; then
  BIN_DIR="${LIBREPCB_BIN_DIR}"
elif [[ -w /usr/local/bin ]]; then
  BIN_DIR=/usr/local/bin
else
  BIN_DIR="${HOME}/.local/bin"
fi

log() { printf '==> %s\n' "$*"; }

# 1. System packages.
if command -v apt-get >/dev/null; then
  SUDO=""
  if [[ "$(id -u)" -ne 0 ]]; then SUDO="sudo"; fi
  log "Installing system packages"
  ${SUDO} apt-get update -qq
  ${SUDO} env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    build-essential pkg-config git curl ca-certificates \
    xvfb xauth libgl1 libegl1 libfontconfig1 libx11-xcb1 libxcb1 >/dev/null
  # Java for FreeRouting (optional: its tests are skipped without it).
  ${SUDO} env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    openjdk-25-jre-headless >/dev/null ||
    log "Warning: could not install openjdk-25-jre-headless"
else
  log "No apt-get: skipping system packages (need a C toolchain, git, curl, Xvfb)"
fi

# 2. Rust toolchain.
if ! command -v cargo >/dev/null; then
  log "Installing Rust (rustup, stable)"
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
    --component rustfmt,clippy
  # shellcheck disable=SC1091
  source "${HOME}/.cargo/env"
fi
log "$(cargo --version)"

# 3. Upstream checkout.
if [[ ! -d "${UPSTREAM_DIR}/.git" ]]; then
  log "Cloning upstream LibrePCB into ${UPSTREAM_DIR}"
  mkdir -p "${UPSTREAM_DIR}"
  git -C "${UPSTREAM_DIR}" init -q
  git -C "${UPSTREAM_DIR}" remote add origin "${UPSTREAM_REPO}"
fi
if [[ "$(git -C "${UPSTREAM_DIR}" rev-parse -q --verify HEAD || true)" != "${UPSTREAM_REV}" ]]; then
  log "Checking out upstream ${UPSTREAM_REV}"
  git -C "${UPSTREAM_DIR}" fetch -q --depth 1 origin "${UPSTREAM_REV}"
  git -C "${UPSTREAM_DIR}" checkout -q --detach FETCH_HEAD
fi
log "Updating upstream submodules: ${UPSTREAM_SUBMODULES[*]}"
git -C "${UPSTREAM_DIR}" submodule update -q --init --depth 1 "${UPSTREAM_SUBMODULES[@]}"

# 4. Upstream CLI.
CLI_DIR="${TOOLS_DIR}/librepcb-${CLI_VERSION}-linux-x86_64"
if [[ ! -x "${CLI_DIR}/bin/librepcb-cli" ]]; then
  log "Downloading librepcb-cli ${CLI_VERSION}"
  mkdir -p "${TOOLS_DIR}"
  curl -sSfL -o "${TOOLS_DIR}/${CLI_ARCHIVE}" "${CLI_BASE_URL}/${CLI_ARCHIVE}"
  curl -sSfL -o "${TOOLS_DIR}/sha256sums.txt" "${CLI_BASE_URL}/sha256sums.txt"
  (cd "${TOOLS_DIR}" && grep " ${CLI_ARCHIVE}\$" sha256sums.txt | sha256sum -c --quiet -)
  tar -xzf "${TOOLS_DIR}/${CLI_ARCHIVE}" -C "${TOOLS_DIR}"
  rm "${TOOLS_DIR}/${CLI_ARCHIVE}"
fi
mkdir -p "${BIN_DIR}"
cat >"${BIN_DIR}/librepcb-cli" <<EOF
#!/usr/bin/env bash
# Upstream librepcb-cli ${CLI_VERSION}, run headless (installed by
# librepcb-rs/scripts/cloud-setup.sh). The release build only ships the xcb
# platform plugin, so callers asking for "offscreen" get xcb on Xvfb instead.
export QT_QPA_PLATFORM=xcb
exec xvfb-run -a "${CLI_DIR}/bin/librepcb-cli" "\$@"
EOF
chmod +x "${BIN_DIR}/librepcb-cli"
log "$("${BIN_DIR}/librepcb-cli" --version | head -n 1)"

# 5. FreeRouting.
FREEROUTING_JAR="${TOOLS_DIR}/freerouting-${FREEROUTING_VERSION}.jar"
if [[ ! -f "${FREEROUTING_JAR}" ]]; then
  log "Downloading FreeRouting ${FREEROUTING_VERSION}"
  mkdir -p "${TOOLS_DIR}"
  curl -sSfL -o "${FREEROUTING_JAR}.part" "${FREEROUTING_URL}"
  echo "${FREEROUTING_SHA256}  ${FREEROUTING_JAR}.part" | sha256sum -c --quiet -
  mv "${FREEROUTING_JAR}.part" "${FREEROUTING_JAR}"
fi
JAVA_FOR_FREEROUTING=""
for candidate in "${LIBREPCB_FREEROUTING_JAVA:-}" /usr/lib/jvm/java-2[5-9]-openjdk*/bin/java \
  "$(command -v java || true)"; do
  if [[ -n "${candidate}" && -x "${candidate}" ]] &&
    "${candidate}" -version 2>&1 | grep -qE 'version "(2[5-9]|[3-9][0-9])'; then
    JAVA_FOR_FREEROUTING="${candidate}"
    break
  fi
done
if [[ -n "${JAVA_FOR_FREEROUTING}" ]]; then
  cat >"${BIN_DIR}/freerouting" <<EOF
#!/usr/bin/env bash
# FreeRouting ${FREEROUTING_VERSION}, run headless with Java >= 25 (installed by
# librepcb-rs/scripts/cloud-setup.sh). librepcb-editor finds this launcher in
# the PATH; alternatively set LIBREPCB_FREEROUTING_JAR=${FREEROUTING_JAR}
# (and LIBREPCB_FREEROUTING_JAVA=${JAVA_FOR_FREEROUTING}).
exec "${JAVA_FOR_FREEROUTING}" -Djava.awt.headless=true -jar "${FREEROUTING_JAR}" "\$@"
EOF
  chmod +x "${BIN_DIR}/freerouting"
  log "FreeRouting ${FREEROUTING_VERSION}: ${BIN_DIR}/freerouting (${JAVA_FOR_FREEROUTING})"
else
  log "Warning: no Java >= 25 found, FreeRouting tests will be skipped"
fi

case ":${PATH}:" in
  *":${BIN_DIR}:"*) ;;
  *)
    log "Note: ${BIN_DIR} is not in PATH; set LIBREPCB_CLI=${BIN_DIR}/librepcb-cli"
    log "  and LIBREPCB_FREEROUTING_JAR=${FREEROUTING_JAR}"
    ;;
esac
log "Done. Upstream: ${UPSTREAM_DIR}"
