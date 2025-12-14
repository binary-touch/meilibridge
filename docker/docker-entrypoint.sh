#!/bin/bash
set -e

# Docker entrypoint for MeiliBridge
# Handles environment variable configuration for ports

# Set the API port via environment variable if provided
if [ -n "${MEILIBRIDGE_API_PORT}" ]; then
    export MEILIBRIDGE__API__PORT="${MEILIBRIDGE_API_PORT}"
fi

# Allow setting the bind address
if [ -n "${MEILIBRIDGE_API_HOST}" ]; then
    export MEILIBRIDGE__API__HOST="${MEILIBRIDGE_API_HOST}"
fi

# Warn if metrics port is configured (feature removed)
if [ -n "${MEILIBRIDGE_METRICS_PORT}" ]; then
    echo "WARNING: MEILIBRIDGE_METRICS_PORT is set but metrics endpoints are no longer supported; ignoring."
fi

# Template the configuration file if present
CONFIG_PATH="${MEILIBRIDGE_CONFIG:-/etc/meilibridge/config.yaml}"
if [ -f "${CONFIG_PATH}" ]; then
    TMP_CONFIG="$(mktemp /tmp/meilibridge-config.XXXXXX.yaml)"
    envsubst < "${CONFIG_PATH}" > "${TMP_CONFIG}"
    export MEILIBRIDGE_CONFIG="${TMP_CONFIG}"

    # Rewrite --config arguments to use the rendered file
    if [ "$#" -gt 0 ]; then
        rendered_args=()
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --config)
                    rendered_args+=("--config" "${MEILIBRIDGE_CONFIG}")
                    shift
                    [ "$#" -gt 0 ] && shift
                    ;;
                --config=*)
                    rendered_args+=("--config=${MEILIBRIDGE_CONFIG}")
                    shift
                    ;;
                *)
                    rendered_args+=("$1")
                    shift
                    ;;
            esac
        done
        set -- "${rendered_args[@]}"
    fi
fi

# Log the configuration
echo "Starting MeiliBridge with configuration:"
echo "  API Host: ${MEILIBRIDGE__API__HOST:-0.0.0.0}"
echo "  API Port: ${MEILIBRIDGE__API__PORT:-7701}"
echo "  Config Path: ${MEILIBRIDGE_CONFIG:-${CONFIG_PATH}}"
echo "  Metrics: disabled (removed in >=0.13)"

# Execute MeiliBridge with all arguments passed to the entrypoint
exec meilibridge "$@"
