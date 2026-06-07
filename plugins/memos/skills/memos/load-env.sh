#!/bin/bash
# Environment Loading Library
# Loads the generated Memos plugin config with legacy env fallbacks.
#
# In skill scripts, source as:
#   source "$HOME/.claude-homelab/load-env.sh"

# Prevent direct execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "Error: This library must be sourced, not executed directly" >&2
    exit 1
fi

# Load a generated plugin config, legacy ~/.lab/.env, or old ~/.claude-homelab/.env.
# Usage: load_env_file [/optional/override/path]
load_env_file() {
    local default_file="${XDG_CONFIG_HOME:-$HOME/.config}/lab-memos/config.env"
    local env_file="${1:-$default_file}"

    if [[ ! -f "$env_file" && "$env_file" == "$default_file" && -f "$HOME/.lab/.env" ]]; then
        env_file="$HOME/.lab/.env"
    fi
    if [[ ! -f "$env_file" && "$env_file" == "$default_file" && -f "$HOME/.claude-homelab/.env" ]]; then
        env_file="$HOME/.claude-homelab/.env"
    fi

    if [[ ! -f "$env_file" ]]; then
        echo "ERROR: $env_file not found" >&2
        echo "Configure the Memos plugin or add credentials to ~/.lab/.env" >&2
        return 1
    fi

    set -a
    # shellcheck source=/dev/null
    source "$env_file"
    set +a
}

# Validate that required environment variables are set and non-empty
# Usage: validate_env_vars "VAR1" "VAR2" ...
validate_env_vars() {
    local missing=()
    for var in "$@"; do
        [[ -z "${!var:-}" ]] && missing+=("$var")
    done

    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "ERROR: Missing required variables: ${missing[*]}" >&2
        return 1
    fi
}

# Load and validate service credentials in one call
# Usage: load_service_credentials "service-name" "URL_VAR" "KEY_VAR"
load_service_credentials() {
    local url_var="$2"
    local key_var="$3"

    if [[ -z "${!url_var:-}" ]] || [[ -z "${!key_var:-}" ]]; then
        load_env_file || return 1
    fi

    if [[ "$key_var" == "MEMOS_API_TOKEN" && -z "${MEMOS_API_TOKEN:-}" && -n "${MEMOS_TOKEN:-}" ]]; then
        MEMOS_API_TOKEN="$MEMOS_TOKEN"
        export MEMOS_API_TOKEN
    fi

    validate_env_vars "$url_var" "$key_var"
}
