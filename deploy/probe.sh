#!/bin/sh
# Probes a todo-server deployment from the outside, without credentials. The POSIX twin of probe.ps1:
# the same requests and checks, with curl.
#
#   deploy/probe.sh https://todo.example.com
#   deploy/probe.sh --allow-http http://127.0.0.1:47320
#
# Environment: DELAY (seconds between requests, default 0.25), TIMEOUT (per request, default 15).
# Exit code: 0 if every check passed, 1 if any failed.
set -u

allow_http=0
if [ "${1:-}" = "--allow-http" ]; then allow_http=1; shift; fi
base="${1:-}"
if [ -z "$base" ]; then
    echo "usage: $0 [--allow-http] <base-url>" >&2
    exit 2
fi
base="${base%/}"
case "$base" in
    https://*) https=1 ;;
    http://*)
        https=0
        if [ "$allow_http" -ne 1 ]; then
            echo "$base is not https; pass --allow-http to probe a local server" >&2
            exit 2
        fi
        ;;
    *) echo "the base URL must start with https:// or http://" >&2; exit 2 ;;
esac
rest="${base#*://}"
case "$rest" in */*) echo "the base URL must be an origin, without a path" >&2; exit 2 ;; esac
host="${rest%%:*}"

delay="${DELAY:-0.25}"
timeout="${TIMEOUT:-15}"
failures=0
warnings=0
sent=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

result() { # verdict what detail
    case "$1" in
        FAIL) failures=$((failures + 1)) ;;
        WARN) warnings=$((warnings + 1)) ;;
    esac
    printf '%-4s  %-44s %s\n' "$1" "$2" "$3"
}

# probe METHOD PATH [BODY] [HEADER...]: sets $status, $bytes, and writes headers to $tmp/h, body to $tmp/b.
probe() {
    method="$1"; path="$2"; shift 2
    body=""
    if [ $# -gt 0 ]; then body="$1"; shift; fi
    [ "$sent" -gt 0 ] && sleep "$delay"
    sent=$((sent + 1))
    if [ -n "$body" ]; then
        status="$(curl -sS -o "$tmp/b" -D "$tmp/h" -w '%{http_code}' --max-time "$timeout" \
            -X "$method" -H 'Content-Type: application/json' --data "$body" "$@" "$base$path" 2>"$tmp/e")"
    elif [ "$method" = "HEAD" ]; then
        status="$(curl -sS -o "$tmp/b" -D "$tmp/h" -w '%{http_code}' --max-time "$timeout" -I "$@" "$base$path" 2>"$tmp/e")"
    else
        status="$(curl -sS -o "$tmp/b" -D "$tmp/h" -w '%{http_code}' --max-time "$timeout" \
            -X "$method" "$@" "$base$path" 2>"$tmp/e")"
    fi
    [ -n "$status" ] || status=000
    bytes="$(wc -c <"$tmp/b" | tr -d ' ')"
}

# header NAME: the value of a response header (case-insensitive), or empty.
header() {
    tr -d '\r' <"$tmp/h" | awk -v n="$1" 'BEGIN { IGNORECASE = 1 } tolower($0) ~ "^" tolower(n) ":" { sub(/^[^:]*:[ ]*/, ""); print; exit }'
}

answer() {
    type="$(header content-type | sed 's/;.*//')"
    loc="$(header location)"
    s="$status, $bytes bytes"
    [ -n "$type" ] && s="$s, $type"
    [ -n "$loc" ] && s="$s, -> $loc"
    printf '%s' "$s"
}

echo "Probing $base"
echo
echo "Transport"
if [ "$https" -eq 1 ]; then
    tls="$(curl -sS -o /dev/null -w '%{ssl_verify_result} %{http_version}' --max-time "$timeout" "$base/health" 2>"$tmp/e")"
    sent=$((sent + 1))
    if [ -n "$tls" ] && [ "${tls%% *}" = "0" ]; then
        result PASS "certificate" "verified"
    else
        result FAIL "certificate" "$(cat "$tmp/e")"
    fi
    if curl -sS -o /dev/null --max-time "$timeout" --tlsv1.2 --tls-max 1.1 "$base/health" 2>/dev/null; then
        result FAIL "TLS version" "a TLS 1.1 handshake succeeded"
    else
        result PASS "TLS version" "TLS 1.1 and older refused"
    fi
    sent=$((sent + 1))
    code="$(curl -sS -o /dev/null -D "$tmp/h" -w '%{http_code}' --max-time "$timeout" "http://$host/health" 2>/dev/null)"
    sent=$((sent + 1))
    loc="$(header location)"
    case "$code:$loc" in
        3??:https://*) result PASS "plain HTTP" "redirects to HTTPS ($code)" ;;
        000:*|:*) result PASS "plain HTTP" "not served" ;;
        *) result WARN "plain HTTP" "answers $code without redirecting (turn on Always Use HTTPS)" ;;
    esac
else
    result SKIP "TLS" "http:// base URL (--allow-http)"
fi

echo
echo "Reachable without authentication"
check_public() { # method path [body] note
    probe "$1" "$2" "${3:-}"
    case "$status" in
        000|429|5??) result FAIL "$1 $2" "$(answer)${4:+  ($4)}" ;;
        *) result PASS "$1 $2" "$(answer)${4:+  ($4)}" ;;
    esac
}
probe GET /health
health_status="$status"; health_bytes="$bytes"; cp "$tmp/h" "$tmp/health.h"
result "$( [ "$status" = 000 ] || [ "$status" -ge 500 ] && echo FAIL || echo PASS)" "GET /health" "$(answer)"
probe GET /login
cp "$tmp/h" "$tmp/login.h"
result "$( [ "$status" = 000 ] || [ "$status" -ge 500 ] && echo FAIL || echo PASS)" "GET /login" "$(answer)"
check_public GET /setup
check_public GET /pair
check_public POST /auth/login/start "" "no Origin: refused before a ceremony starts"
check_public POST /auth/login/finish "" "no Origin"
check_public POST /auth/setup/start "" "no Origin"
check_public POST /auth/setup/finish "" "no Origin"
check_public POST /api/v1/pair/start '{}' "empty body: no pairing starts"
check_public POST /api/v1/pair/poll '{}' "empty body"
probe GET /assets/app.css
cp "$tmp/h" "$tmp/css.h"
result "$( [ "$status" = 200 ] && echo PASS || echo FAIL)" "GET /assets/app.css" "$(answer)"
check_public GET /s/AAAAAAAAAAAAAAAAAAAAAA "" "a made-up share token"

echo
echo "Refused without authentication"
check_refused() { # method path [body] [header...]
    m="$1"; p="$2"; b="${3:-}"; shift 2; [ $# -gt 0 ] && shift
    probe "$m" "$p" "$b" "$@"
    loc="$(header location)"
    ok=0
    case "$status" in 401|403|404|405) ok=1 ;; esac
    case "$status:$loc" in 303:/login\?next=*) ok=1 ;; esac
    case "$m:$status" in TRACE:400|TRACE:501|BREW:400|BREW:501) ok=1 ;; esac
    if [ "$ok" -eq 1 ]; then result PASS "$m $p" "$(answer)"; else result FAIL "$m $p" "$(answer)"; fi
}
uuid=00000000-0000-4000-8000-000000000000
check_refused GET /api/v1/lists
cp "$tmp/h" "$tmp/api.h"; api_status="$status"
check_refused GET /api/v1/lists "" -H "Authorization: Bearer tda_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
check_refused GET "/api/v1/tasks/$uuid"
check_refused GET "/api/v1/tasks?query="
check_refused GET /api/v1/events "" -H "Accept: text/event-stream"
check_refused GET /api/v1/openapi.json
check_refused POST /api/v1/commands '{}'
check_refused POST /api/v1/events/1/undo '{}'
check_refused POST /mcp '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' -H "Accept: application/json, text/event-stream"
check_refused GET /mcp
check_refused GET /
check_refused GET /admin
check_refused GET /admin/agents
check_refused POST /admin/agents '{}'
check_refused POST /auth/step-up/start '{}'
check_refused POST /pair/approve '{}'
check_refused GET /api/v1/nope
check_refused GET /nope
check_refused GET /.env
check_refused PUT /health '{}'
check_refused DELETE /api/v1/lists
check_refused OPTIONS /api/v1/lists
check_refused TRACE /
check_refused BREW /admin

echo
echo "Headers and contents"
if [ "$health_status" = 200 ] && [ "$health_bytes" = 0 ]; then
    result PASS "/health" "empty 200"
else
    result FAIL "/health" "want an empty 200, got $health_status with $health_bytes bytes"
fi
cp "$tmp/health.h" "$tmp/h"
server="$(header server)"
leaky="$(tr -d '\r' <"$tmp/h" | grep -i -E '^(x-powered-by|[^:]*version[^:]*):' || true)"
if [ -z "$leaky" ] && { [ -z "$server" ] || [ "$server" = cloudflare ]; }; then
    result PASS "/health reveals nothing" "${server:+Server: $server (the edge)}"
else
    result FAIL "/health reveals nothing" "Server: '$server' $leaky"
fi

hsts() { # what headers-file
    if [ "$https" -ne 1 ]; then result SKIP "HSTS on $1" "http:// base URL"; return; fi
    cp "$2" "$tmp/h"
    v="$(header strict-transport-security)"
    age="$(printf '%s' "$v" | sed -n 's/.*max-age=\([0-9]*\).*/\1/p')"
    if [ -n "$age" ] && [ "$age" -ge 15552000 ]; then result PASS "HSTS on $1" "$v"; else result FAIL "HSTS on $1" "got '$v'"; fi
}
hsts /health "$tmp/health.h"

cp "$tmp/login.h" "$tmp/h"
csp="$(header content-security-policy)"
case "$csp" in *"frame-ancestors 'none'"*) result PASS "/login CSP frame-ancestors" "$csp" ;; *) result FAIL "/login CSP frame-ancestors" "$csp" ;; esac
case "$csp" in *"default-src 'self'"*|*"default-src 'none'"*) result PASS "/login CSP default-src" "" ;; *) result FAIL "/login CSP default-src" "$csp" ;; esac
v="$(header x-frame-options)"; [ "$v" = DENY ] && result PASS "/login X-Frame-Options" "$v" || result FAIL "/login X-Frame-Options" "$v"
v="$(header x-content-type-options)"; [ "$v" = nosniff ] && result PASS "/login X-Content-Type-Options" "$v" || result FAIL "/login X-Content-Type-Options" "$v"
v="$(header referrer-policy)"
case "$v" in no-referrer|same-origin|strict-origin) result PASS "/login Referrer-Policy" "$v" ;; *) result FAIL "/login Referrer-Policy" "$v" ;; esac
hsts /login "$tmp/login.h"

cp "$tmp/api.h" "$tmp/h"
v="$(header www-authenticate)"
[ "$api_status" = 401 ] && [ "$v" = Bearer ] && result PASS "API 401" "WWW-Authenticate: $v" || result FAIL "API 401" "$api_status, WWW-Authenticate: $v"
v="$(header x-content-type-options)"; [ "$v" = nosniff ] && result PASS "API nosniff" "$v" || result FAIL "API nosniff" "$v"
hsts "API errors" "$tmp/api.h"

cp "$tmp/css.h" "$tmp/h"
t="$(header content-type)"; n="$(header x-content-type-options)"
case "$t:$n" in text/css*:nosniff) result PASS "static asset typed" "$t, $n" ;; *) result FAIL "static asset typed" "$t, $n" ;; esac

echo
echo "$sent requests, $failures failed, $warnings warnings"
[ "$failures" -eq 0 ]
