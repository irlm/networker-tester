#!/usr/bin/env bash
# Probe all five LagHound sample services and assert contract v1 across languages.
#
# For each service it checks two things:
#   1. GET /laghound/health WITH the token  -> HTTP 200 and body contains "contract":"v1"
#   2. GET /laghound/health WITHOUT the token -> HTTP 404 (the routes are invisible)
#
# The second check is the cross-language invisibility proof: a missing/wrong
# token yields the exact same bare 404 a nonexistent route would — no LagHound
# fingerprint. Prints a PASS/FAIL table and exits non-zero if anything failed.
#
# Usage:
#   ./probe-all.sh                 # localhost, token from env/.env or the demo default
#   HOST=1.2.3.4 ./probe-all.sh    # a remote VM running the compose harness
set -u

HOST="${HOST:-localhost}"
TOKEN="${LAGHOUND_TOKEN:-demo-token-laghound}"

# lang:port pairs (bash 3.2 friendly — no associative arrays).
SERVICES="csharp:8081 js:8082 python:8083 rust:8084 go:8085"

pass=0
fail=0

printf '%-10s %-7s %-14s %-16s %s\n' "LANG" "PORT" "AUTH(->200/v1)" "NOAUTH(->404)" "RESULT"
printf '%s\n' "------------------------------------------------------------------------"

for entry in $SERVICES; do
  lang="${entry%%:*}"
  port="${entry##*:}"
  base="http://${HOST}:${port}/laghound/health"

  # 1) With the token: expect 200 + contract:v1 in the JSON body.
  body_auth="$(curl -fsS -m 10 -H "X-LagHound-Token: ${TOKEN}" "$base" 2>/dev/null)"
  code_auth="$(curl -s -o /dev/null -m 10 -w '%{http_code}' -H "X-LagHound-Token: ${TOKEN}" "$base" 2>/dev/null)"
  auth_ok="no"
  if [ "$code_auth" = "200" ] && printf '%s' "$body_auth" | grep -q '"contract"[[:space:]]*:[[:space:]]*"v1"'; then
    auth_ok="yes"
  fi

  # 2) Without the token: expect a bare 404 (invisible).
  code_noauth="$(curl -s -o /dev/null -m 10 -w '%{http_code}' "$base" 2>/dev/null)"
  noauth_ok="no"
  if [ "$code_noauth" = "404" ]; then
    noauth_ok="yes"
  fi

  if [ "$auth_ok" = "yes" ] && [ "$noauth_ok" = "yes" ]; then
    result="PASS"
    pass=$((pass + 1))
  else
    result="FAIL"
    fail=$((fail + 1))
  fi

  printf '%-10s %-7s %-14s %-16s %s\n' \
    "$lang" "$port" "${code_auth}/${auth_ok}" "${code_noauth}/${noauth_ok}" "$result"
done

printf '%s\n' "------------------------------------------------------------------------"
printf 'Total: %d PASS, %d FAIL\n' "$pass" "$fail"

[ "$fail" -eq 0 ]
