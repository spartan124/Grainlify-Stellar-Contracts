#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOROBAN_DIR="$ROOT_DIR/soroban"
CARGO_TOML="$SOROBAN_DIR/Cargo.toml"
CARGO_LOCK="$SOROBAN_DIR/Cargo.lock"

SDK_VERSION=""
NETWORK=""
CONTRACT="escrow"
DEPLOY_CHECK=""
STARTED_LOCAL="false"

CARGO_TOML_BACKUP=""
CARGO_LOCK_BACKUP=""

log() {
    echo "[matrix] $*"
}

die() {
    echo "[matrix][error] $*" >&2
    exit 1
}

show_usage() {
    cat <<'USAGE'
Usage:
  ./scripts/run_contract_matrix.sh --sdk-version <version> --network <local|testnet> [options]

Required:
  --sdk-version <version>   Soroban SDK version to test (example: 23, 23.0.3)
  --network <network>       Network to validate against: local | testnet

Optional:
  --contract <name>         Contract wasm to validate (default: escrow)
  --deploy-check <mode>     skip | dry-run | actual
                            default: actual for local, dry-run for testnet
  -h, --help                Show help

Examples:
  ./scripts/run_contract_matrix.sh --sdk-version 23 --network local
  ./scripts/run_contract_matrix.sh --sdk-version 23 --network testnet
  ./scripts/run_contract_matrix.sh --sdk-version 23.0.3 --network local --contract program-escrow
USAGE
}

cleanup() {
    if [[ -n "$CARGO_TOML_BACKUP" && -f "$CARGO_TOML_BACKUP" ]]; then
        cp "$CARGO_TOML_BACKUP" "$CARGO_TOML"
    fi

    if [[ -n "$CARGO_LOCK_BACKUP" && -f "$CARGO_LOCK_BACKUP" ]]; then
        cp "$CARGO_LOCK_BACKUP" "$CARGO_LOCK"
    fi

    if [[ "$STARTED_LOCAL" == "true" ]]; then
        log "Stopping local sandbox started by matrix runner"
        stellar container stop local >/dev/null 2>&1 || true
    fi
}

trap cleanup EXIT

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --sdk-version)
                SDK_VERSION="$2"
                shift 2
                ;;
            --network)
                NETWORK="$2"
                shift 2
                ;;
            --contract)
                CONTRACT="$2"
                shift 2
                ;;
            --deploy-check)
                DEPLOY_CHECK="$2"
                shift 2
                ;;
            -h|--help)
                show_usage
                exit 0
                ;;
            *)
                die "Unknown argument: $1"
                ;;
        esac
    done
}

validate_args() {
    [[ -n "$SDK_VERSION" ]] || die "--sdk-version is required"
    [[ -n "$NETWORK" ]] || die "--network is required"

    case "$NETWORK" in
        local|testnet) ;;
        *)
            die "Invalid network: $NETWORK (valid: local, testnet)"
            ;;
    esac

    if [[ -z "$DEPLOY_CHECK" ]]; then
        if [[ "$NETWORK" == "local" ]]; then
            DEPLOY_CHECK="actual"
        else
            DEPLOY_CHECK="dry-run"
        fi
    fi

    case "$DEPLOY_CHECK" in
        skip|dry-run|actual) ;;
        *)
            die "Invalid --deploy-check: $DEPLOY_CHECK (valid: skip, dry-run, actual)"
            ;;
    esac
}

check_dependencies() {
    command -v cargo >/dev/null 2>&1 || die "cargo is required"
    command -v rustup >/dev/null 2>&1 || die "rustup is required"
    command -v stellar >/dev/null 2>&1 || die "stellar CLI is required"
    command -v jq >/dev/null 2>&1 || die "jq is required"
    command -v curl >/dev/null 2>&1 || die "curl is required"

    [[ -f "$CARGO_TOML" ]] || die "Missing $CARGO_TOML"
    [[ -f "$CARGO_LOCK" ]] || die "Missing $CARGO_LOCK"
    [[ -f "$ROOT_DIR/scripts/deploy.sh" ]] || die "Missing scripts/deploy.sh"
    [[ -f "$ROOT_DIR/scripts/config/${NETWORK}.env" ]] || die "Missing scripts/config/${NETWORK}.env"
}

backup_workspace_files() {
    CARGO_TOML_BACKUP="$(mktemp)"
    CARGO_LOCK_BACKUP="$(mktemp)"

    cp "$CARGO_TOML" "$CARGO_TOML_BACKUP"
    cp "$CARGO_LOCK" "$CARGO_LOCK_BACKUP"
}

set_sdk_version() {
    log "Setting soroban-sdk version to $SDK_VERSION"
    sed -i.bak -E "s/^soroban-sdk = \".*\"/soroban-sdk = \"=$SDK_VERSION\"/" "$CARGO_TOML"
    rm -f "${CARGO_TOML}.bak"

    grep -q "soroban-sdk = \"=$SDK_VERSION\"" "$CARGO_TOML" || die "Failed to set soroban-sdk version"
}

load_network_config() {
    # shellcheck disable=SC1090
    source "$ROOT_DIR/scripts/config/${NETWORK}.env"

    : "${SOROBAN_RPC_URL:?missing SOROBAN_RPC_URL}"
    : "${SOROBAN_NETWORK:?missing SOROBAN_NETWORK}"
    : "${DEPLOYER_IDENTITY:?missing DEPLOYER_IDENTITY}"
}

ensure_local_network() {
    command -v docker >/dev/null 2>&1 || die "docker is required for local network checks"

    if stellar network health --network local >/dev/null 2>&1; then
        log "Local network already healthy"
        return
    fi

    log "Starting local sandbox"
    stellar container start local --limits testnet >/dev/null
    STARTED_LOCAL="true"

    stellar network rm local >/dev/null 2>&1 || true
    stellar network add --rpc-url http://localhost:8000/rpc --network-passphrase "Standalone Network ; February 2017" local >/dev/null

    for _ in $(seq 1 24); do
        if stellar network health --network local >/dev/null 2>&1; then
            log "Local network is healthy"
            return
        fi
        sleep 5
    done

    die "Local sandbox did not become healthy in time"
}

ensure_identity() {
    if ! stellar keys address "$DEPLOYER_IDENTITY" >/dev/null 2>&1; then
        log "Creating identity: $DEPLOYER_IDENTITY"
        stellar keys generate "$DEPLOYER_IDENTITY" >/dev/null
    fi

    if [[ "$NETWORK" == "local" ]]; then
        local addr
        addr="$(stellar keys address "$DEPLOYER_IDENTITY")"
        log "Funding local identity: $DEPLOYER_IDENTITY"

        if ! curl -fsS "${FRIENDBOT_URL}?addr=${addr}" >/dev/null; then
            log "Local friendbot funding returned non-zero; continuing because the account may already exist"
        fi
    else
        log "Funding testnet identity: $DEPLOYER_IDENTITY"
        stellar keys fund "$DEPLOYER_IDENTITY" --network testnet >/dev/null || true
    fi
}

build_and_test_workspace() {
    log "Running cargo resolution for sdk=$SDK_VERSION"
    (
        cd "$SOROBAN_DIR"
        export CARGO_INCREMENTAL=0
        # Only update soroban-sdk and its direct/transitive deps — NOT the entire
        # dep graph. A full `cargo update` resolves ed25519-dalek to v3.x which
        # causes a trait-mismatch with soroban-env-host v23. By targeting only
        # soroban-sdk we preserve the rest of the lockfile (including the
        # ed25519-dalek 2.2.0 pin) while still picking up the requested SDK version.
        cargo update soroban-sdk
        # Safety net: if ed25519-dalek somehow bumped to 3.x, downgrade it.
        cargo update -p ed25519-dalek --precise 2.2.0 2>/dev/null || true
        cargo test --workspace
        cargo build --release --target wasm32-unknown-unknown
    )
}

run_deploy_check() {
    if [[ "$DEPLOY_CHECK" == "skip" ]]; then
        log "Skipping deploy validation"
        return
    fi

    local wasm_path="$ROOT_DIR/soroban/target/wasm32-unknown-unknown/release/${CONTRACT}.wasm"
    [[ -f "$wasm_path" ]] || die "Missing wasm artifact: $wasm_path"

    local -a cmd=("$ROOT_DIR/scripts/deploy.sh" "$wasm_path" "-n" "$NETWORK" "-v")

    if [[ "$DEPLOY_CHECK" == "dry-run" ]]; then
        cmd+=("--dry-run")
    fi

    log "Running deploy validation (${DEPLOY_CHECK}) for contract=${CONTRACT} network=${NETWORK}"
    "${cmd[@]}"
}

main() {
    parse_args "$@"
    validate_args
    check_dependencies
    backup_workspace_files

    echo "MATRIX START sdk=${SDK_VERSION} network=${NETWORK} contract=${CONTRACT} deploy_check=${DEPLOY_CHECK}"

    load_network_config

    if [[ "$NETWORK" == "local" ]]; then
        ensure_local_network
    fi

    ensure_identity
    set_sdk_version
    build_and_test_workspace
    run_deploy_check

    echo "MATRIX RESULT sdk=${SDK_VERSION} network=${NETWORK} contract=${CONTRACT} status=PASS"
}

main "$@"
rc=$?

if [[ $rc -eq 0 ]]; then
    exit 0
fi

echo "MATRIX RESULT sdk=${SDK_VERSION:-unknown} network=${NETWORK:-unknown} contract=${CONTRACT:-unknown} status=FAIL exit_code=${rc}" >&2
exit "$rc"
