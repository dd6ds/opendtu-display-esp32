#!/usr/bin/env bash
# =============================================================================
#  build_flash.sh — Setup, build and flash for ESP32-2432S028 (CYD)
#  Project: opendtu-display-esp32
#
#  What this script does:
#    1. Installs all required system packages
#    2. Installs Rust + rustup (if missing)
#    3. Installs the ESP Rust toolchain via espup (xtensa-esp32-espidf)
#    4. Installs ldproxy and espflash via cargo
#    5. Checks serial port permissions (dialout group)
#    6. Writes .cargo/config.toml  ← fixes the --ldproxy-linker build error
#    7. Asks for WiFi SSID and password → patches src/main.rs
#    8. Builds the project in release mode
#    9. Detects the USB serial port and flashes the device
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RS="$SCRIPT_DIR/src/main.rs"
BINARY="$SCRIPT_DIR/target/xtensa-esp32-espidf/release/opendtu-display-esp32"

# ── Colours ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

step()  { echo -e "\n${BLUE}${BOLD}▶ $*${NC}"; }
ok()    { echo -e "${GREEN}✓ $*${NC}"; }
warn()  { echo -e "${YELLOW}⚠ $*${NC}"; }
err()   { echo -e "${RED}✗ $*${NC}"; exit 1; }
info()  { echo -e "${CYAN}  $*${NC}"; }

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║   ESP32-2432S028 (CYD) — Build & Flash               ║${NC}"
echo -e "${BOLD}║   OpenDTU Display                                     ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════╝${NC}"
echo ""

# ── 1. System packages ────────────────────────────────────────────────────────
step "Checking system packages..."

REQUIRED_PKGS=(
    git curl wget
    python3 python3-pip python3-venv
    libssl-dev pkg-config
    build-essential
    libudev-dev      # needed by espflash for serial port access
    libpython3-dev   # needed by ESP-IDF build system
)

MISSING_PKGS=()
for pkg in "${REQUIRED_PKGS[@]}"; do
    dpkg -s "$pkg" &>/dev/null || MISSING_PKGS+=("$pkg")
done

if [ "${#MISSING_PKGS[@]}" -gt 0 ]; then
    warn "Installing missing packages: ${MISSING_PKGS[*]}"
    sudo apt-get update -qq
    sudo apt-get install -y "${MISSING_PKGS[@]}"
fi
ok "System packages ready"

# ── 2. Rustup ─────────────────────────────────────────────────────────────────
step "Checking Rust / rustup..."

if ! command -v rustup &>/dev/null; then
    warn "rustup not found — installing..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --no-modify-path
    export PATH="$HOME/.cargo/bin:$PATH"
fi

# Make sure cargo is on PATH for this session
export PATH="$HOME/.cargo/bin:$PATH"
ok "rustup $(rustup --version 2>&1 | head -1)"

# ── 3. ESP Rust toolchain (espup) ─────────────────────────────────────────────
step "Checking ESP Rust toolchain (xtensa-esp32-espidf)..."

if ! rustup toolchain list 2>/dev/null | grep -q "^esp"; then
    warn "ESP toolchain not found — installing via espup (this takes a few minutes)..."

    if ! command -v espup &>/dev/null; then
        info "Installing espup..."
        cargo install espup
    fi

    espup install
    ok "ESP toolchain installed"
else
    ok "ESP toolchain already present"
fi

# Source the ESP environment variables (LIBCLANG_PATH etc.)
if [ -f "$HOME/export-esp.sh" ]; then
    # shellcheck disable=SC1091
    source "$HOME/export-esp.sh"
    ok "ESP environment sourced (~/.export-esp.sh)"
else
    warn "~/.export-esp.sh not found — espup may not have completed correctly"
fi

# ── 4. ldproxy ────────────────────────────────────────────────────────────────
step "Checking ldproxy (linker proxy for ESP-IDF)..."

if ! command -v ldproxy &>/dev/null; then
    warn "ldproxy not found — installing..."
    cargo install ldproxy
fi
ok "ldproxy $(ldproxy --version 2>/dev/null || echo 'installed')"

# ── 5. espflash ───────────────────────────────────────────────────────────────
step "Checking espflash (flash tool)..."

if ! command -v espflash &>/dev/null; then
    warn "espflash not found — installing..."
    cargo install espflash
fi
ok "espflash $(espflash --version 2>/dev/null | head -1)"

# ── 5b. Serial port permissions (dialout group) ───────────────────────────────
step "Checking serial port permissions..."

if ! groups "$USER" | grep -q '\bdialout\b'; then
    warn "User '$USER' is not in the 'dialout' group."
    sudo usermod -aG dialout "$USER"
    warn "If flashing fails with 'permission denied', run: newgrp dialout"
else
    ok "User '$USER' is in the dialout group"
fi

# ── 6. Cargo project config (.cargo/config.toml) ──────────────────────────────
step "Writing .cargo/config.toml..."

# This file is NOT committed to the repo but is REQUIRED for the build.
#
# ROOT CAUSE of the build error:
#   embuild (used by esp-idf-sys) always injects two flags into every link step:
#     --ldproxy-linker <real-gcc>   which actual linker to call
#     --ldproxy-cwd   <idf-build>   working directory for the linker
#   These are ONLY understood by the `ldproxy` shim. Without this config.toml
#   Cargo calls xtensa-esp32-elf-gcc directly and gcc errors out:
#     "unrecognized command-line option '--ldproxy-linker'"
#
# We re-write this file on every run so a fresh clone always gets it.

mkdir -p "$SCRIPT_DIR/.cargo"

cat > "$SCRIPT_DIR/.cargo/config.toml" << 'CARGO_CONFIG'
# Auto-generated by build_flash.sh — required, do not delete.
# Without this file the build fails with:
#   "unrecognized command-line option '--ldproxy-linker'"

[target.xtensa-esp32-espidf]
linker = "ldproxy"

[unstable]
build-std = ["std", "panic_abort"]

[env]
ESP_IDF_VERSION = "v5.2.3"
CARGO_CONFIG

ok ".cargo/config.toml written"

# ── 7. WiFi credentials ───────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}──────────────────────────────────────────────────────${NC}"
echo -e "${BOLD}  WiFi Configuration${NC}"
echo -e "${BOLD}──────────────────────────────────────────────────────${NC}"

CURRENT_SSID=$(grep -oP '(?<=const WIFI_SSID:\s{3}&str = ")[^"]+' "$MAIN_RS" || echo "")
CURRENT_PASS=$(grep -oP '(?<=const WIFI_PASS:\s{3}&str = ")[^"]+' "$MAIN_RS" || echo "")

[ -n "$CURRENT_SSID" ] && info "Current SSID    : $CURRENT_SSID"
[ -n "$CURRENT_PASS" ] && info "Current Password: ********"
echo ""

read -rp "  WiFi SSID     [${CURRENT_SSID}]: " INPUT_SSID
read -rsp "  WiFi Password [leave blank to keep]: " INPUT_PASS
echo ""

WIFI_SSID="${INPUT_SSID:-$CURRENT_SSID}"
WIFI_PASS="${INPUT_PASS:-$CURRENT_PASS}"

[ -z "$WIFI_SSID" ] && err "WiFi SSID cannot be empty."

echo -e "${BOLD}──────────────────────────────────────────────────────${NC}"
echo ""

# ── 8. Patch src/main.rs with WiFi credentials ────────────────────────────────
step "Patching WiFi credentials in src/main.rs..."

WIFI_SSID="$WIFI_SSID" WIFI_PASS="$WIFI_PASS" MAIN_RS="$MAIN_RS" \
python3 << 'PYEOF'
import re, os
main_rs  = os.environ["MAIN_RS"]
ssid     = os.environ["WIFI_SSID"]
password = os.environ["WIFI_PASS"]
content = open(main_rs).read()
content = re.sub(r'const WIFI_SSID:\s+&str\s*=\s*"[^"]*";',
    f'const WIFI_SSID:   &str = "{ssid}";', content)
content = re.sub(r'const WIFI_PASS:\s+&str\s*=\s*"[^"]*";',
    f'const WIFI_PASS:   &str = "{password}";', content)
open(main_rs, "w").write(content)
print(f'  SSID set to: {ssid}')
PYEOF

ok "WiFi credentials patched"

# ── 9. Build ──────────────────────────────────────────────────────────────────
step "Building for ESP32 (xtensa-esp32-espidf, release)..."
info "This may take several minutes on first build (ESP-IDF is compiled from source)"
echo ""

cd "$SCRIPT_DIR"

# Source ESP environment (sets LIBCLANG_PATH and xtensa toolchain PATH)
if [ -f "$HOME/export-esp.sh" ]; then
    # shellcheck disable=SC1091
    source "$HOME/export-esp.sh"
    ok "ESP environment sourced"
else
    err "~/export-esp.sh not found — run 'espup install' first"
fi

# Belt-and-suspenders: also export the linker via env var.
# .cargo/config.toml (written in step 6) is the primary fix, but this env var
# takes priority over config.toml and guarantees the correct linker is used
# even if Cargo fails to locate the config file through workspace discovery.
LDPROXY_BIN="$(command -v ldproxy 2>/dev/null || echo "$HOME/.cargo/bin/ldproxy")"
if [ ! -x "$LDPROXY_BIN" ]; then
    err "ldproxy not found at '$LDPROXY_BIN'. Make sure 'cargo install ldproxy' succeeded."
fi
export CARGO_TARGET_XTENSA_ESP32_ESPIDF_LINKER="$LDPROXY_BIN"
info "Linker : $LDPROXY_BIN"

cargo +esp build --release --target xtensa-esp32-espidf -Z build-std=std,panic_abort

echo ""
ok "Build successful → $BINARY"

# ── 10. Detect serial port ────────────────────────────────────────────────────
step "Detecting ESP32 serial port..."

PORTS=()
for p in /dev/ttyUSB* /dev/ttyACM*; do
    [ -e "$p" ] && PORTS+=("$p")
done

if [ "${#PORTS[@]}" -eq 0 ]; then
    warn "No USB serial port detected."
    info "Make sure the ESP32-2432S028 is connected via USB and drivers are loaded."
    info "Common USB-serial chips on this board: CH340, CP2102"
    info "Check with: ls /dev/ttyUSB* /dev/ttyACM*"
    read -rp "  Enter port manually (e.g. /dev/ttyUSB0): " FLASH_PORT
    [ -z "$FLASH_PORT" ] && err "No port specified — cannot flash."
elif [ "${#PORTS[@]}" -eq 1 ]; then
    FLASH_PORT="${PORTS[0]}"
    ok "Auto-detected port: $FLASH_PORT"
else
    echo "  Multiple serial ports found:"
    for i in "${!PORTS[@]}"; do
        echo "    [$i] ${PORTS[$i]}"
    done
    read -rp "  Select port number [0]: " SEL
    SEL="${SEL:-0}"
    FLASH_PORT="${PORTS[$SEL]}"
    ok "Selected port: $FLASH_PORT"
fi

# ── 11. Flash ─────────────────────────────────────────────────────────────────
step "Flashing ESP32-2432S028 on $FLASH_PORT ..."
info "The device will reboot automatically after flashing."
info "Press Ctrl+C to exit the serial monitor."
echo ""

espflash flash \
    --chip  esp32 \
    --port  "$FLASH_PORT" \
    --baud  921600 \
    --monitor \
    "$BINARY"

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}${BOLD}╔══════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}${BOLD}║   All done! Device is running.                       ║${NC}"
echo -e "${GREEN}${BOLD}╚══════════════════════════════════════════════════════╝${NC}"
echo ""
