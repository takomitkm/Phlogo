#!/usr/bin/env bash
# Container-runtime detection shared by backup/restore/ops scripts (p7-01, podman-first).
# Resolution order: podman compose -> podman-compose -> docker compose.
# Usage: source this file, then call `runtime_cmd` (prints the command words) and
# `runtime_compose <args...>` to run a compose subcommand.

runtime_cmd() {
  if command -v podman >/dev/null 2>&1; then
    if podman compose version >/dev/null 2>&1; then
      COMPOSE=(podman compose)
    elif command -v podman-compose >/dev/null 2>&1; then
      COMPOSE=(podman-compose)
    elif command -v docker >/dev/null 2>&1; then
      COMPOSE=(docker compose)
    else
      echo "error: podman present but neither 'podman compose' nor podman-compose is available" >&2
      return 1
    fi
  elif command -v docker >/dev/null 2>&1; then
    COMPOSE=(docker compose)
  else
    echo "error: no container runtime found (want podman, fallback docker)" >&2
    return 1
  fi
  printf '%s\n' "${COMPOSE[@]}"
}

runtime_compose() {
  local -a COMPOSE
  COMPOSE=($(runtime_cmd)) || return 1
  "${COMPOSE[@]}" "$@"
}
