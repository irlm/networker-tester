// Package laghound embeds the LagHound diagnostic endpoint (contract v1)
// into any net/http application.
//
// It mounts five routes under a configurable prefix (default "/laghound"):
// /health, /echo, /download, /upload, and /info — all behind a shared token.
// Every response carries a Server-Timing header so the LagHound tester fleet
// can split total request time into DNS, TCP, TLS, network transfer, and
// server processing.
//
// The authoritative spec is docs/sdk/contract-v1.md; the machine-readable
// twin (which the conformance tests in this package pin to) is
// shared/sdk-contract-v1.json.
//
// Zero third-party dependencies: stdlib only.
package laghound

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"os"
	"strings"
	"sync/atomic"
	"time"
)

// SDK identity (reported on /health and /info).
const (
	// ContractVersion is the endpoint contract this SDK implements.
	ContractVersion = "v1"
	// Version is the SDK package version (semver).
	Version = "1.0.0"

	sdkLang = "go"
)

// Contract constants — see shared/sdk-contract-v1.json.
const (
	// DefaultPrefix is the route prefix used when Config.Prefix is empty.
	DefaultPrefix = "/laghound"
	// DefaultCapBytes is the default download/upload cap (4 MiB).
	DefaultCapBytes = 4 << 20
	// AbsoluteMaxBytes is the hard payload ceiling (32 MiB). Config values
	// above it are clamped; it is not configurable.
	AbsoluteMaxBytes = 32 << 20

	echoRequestBodyMax   = 64 << 10
	chunkSize            = 64 << 10
	tokenMinBytes        = 16
	perIPTableMaxEntries = 10000

	envKillSwitch = "LAGHOUND_DISABLED"
	envToken      = "LAGHOUND_TOKEN"
	headerToken   = "X-LagHound-Token"
	headerBytes   = "X-LagHound-Bytes"
	cacheControl  = "no-store, no-cache, must-revalidate"
)

// Configuration errors returned by New. Handler converts any of these into a
// fail-closed handler that answers every request with a bare 404.
var (
	// ErrNoToken means no token was provided via Config or LAGHOUND_TOKEN.
	ErrNoToken = errors.New("laghound: no token configured (set Config.Token or LAGHOUND_TOKEN); refusing to mount open routes")
	// ErrTokenTooShort means a configured token is under the 16-byte minimum.
	ErrTokenTooShort = errors.New("laghound: token shorter than 16 bytes")
	// ErrBadPrefix means Config.Prefix does not start with "/" or has a trailing "/".
	ErrBadPrefix = errors.New("laghound: prefix must start with '/' and have no trailing '/'")
	// ErrBadCap means a negative byte cap was configured.
	ErrBadCap = errors.New("laghound: byte caps must be non-negative")
	// ErrBadBudget means Config.ByteBudget has a non-positive Bytes or WindowS.
	ErrBadBudget = errors.New("laghound: byte budget requires positive Bytes and WindowS")
)

// Rate is a token-bucket rate limit: RPS sustained, Burst peak.
type Rate struct {
	RPS   float64
	Burst int
}

// ByteBudget is an optional sampling budget for the transfer routes: once
// Bytes have been transferred within a window of WindowS seconds, /download
// and /upload answer 429 with Retry-After set to the window remainder.
type ByteBudget struct {
	Bytes   int64
	WindowS int
}

// Config configures the LagHound handler. The zero value of every field
// except Token falls back to the contract default.
type Config struct {
	// Token is the shared secret (min 16 bytes). If empty (and PreviousToken
	// is empty), the LAGHOUND_TOKEN environment variable is used. Without any
	// token the SDK refuses to mount (fail-closed).
	Token string
	// PreviousToken optionally holds the prior token during zero-downtime
	// rotation. Both tokens are compared in constant time.
	PreviousToken string
	// Prefix is the mount prefix. Default "/laghound". Must start with "/",
	// no trailing slash.
	Prefix string
	// AppName is an optional label echoed on /health and /info. Never
	// auto-derived from the host app.
	AppName string

	// DownloadCapBytes caps /download payloads. Default 4 MiB, clamped to 32 MiB.
	DownloadCapBytes int64
	// UploadCapBytes caps /upload payloads. Default 4 MiB, clamped to 32 MiB.
	UploadCapBytes int64

	// RatePerIP is the per-client-IP token bucket. Default 10 req/s, burst 20.
	RatePerIP Rate
	// RateGlobal is the process-wide token bucket. Default 50 req/s, burst 100.
	RateGlobal Rate
	// MaxConcurrent caps in-flight LagHound requests. Default 8.
	MaxConcurrent int
	// MaxConcurrentTransfers caps in-flight /download + /upload. Default 2.
	MaxConcurrentTransfers int
	// ByteBudget optionally enables the transfer byte budget. Off by default.
	ByteBudget *ByteBudget

	// DisableEcho, DisableDownload, DisableUpload, DisableInfo remove
	// individual routes; disabled routes answer bare 404 and report false in
	// the /health capability map. /health itself is always enabled.
	DisableEcho     bool
	DisableDownload bool
	DisableUpload   bool
	DisableInfo     bool
}

// routeSet is the capability map reported on /health and /info.
type routeSet struct {
	Health   bool `json:"health"`
	Echo     bool `json:"echo"`
	Download bool `json:"download"`
	Upload   bool `json:"upload"`
	Info     bool `json:"info"`
}

type handler struct {
	prefix      string
	appName     string
	tokenHashes [][sha256.Size]byte
	downloadCap int64
	uploadCap   int64
	ratePerIP   Rate
	rateGlobal  Rate
	budgetCfg   *ByteBudget
	enabled     routeSet

	maxConcurrent int32
	maxTransfers  int32
	inflight      atomic.Int32
	transfers     atomic.Int32

	perIP  *ipLimiter
	global *lockedBucket
	budget *budget

	start      time.Time
	healthPre  []byte
	healthPost []byte
}

// New builds the LagHound http.Handler, validating the configuration.
// It fails closed: without a usable token it returns an error rather than
// mounting open routes.
func New(cfg Config) (http.Handler, error) {
	tokens := make([]string, 0, 2)
	if cfg.Token != "" {
		tokens = append(tokens, cfg.Token)
	}
	if cfg.PreviousToken != "" {
		tokens = append(tokens, cfg.PreviousToken)
	}
	if len(tokens) == 0 {
		if t := os.Getenv(envToken); t != "" {
			tokens = append(tokens, t)
		}
	}
	if len(tokens) == 0 {
		return nil, ErrNoToken
	}
	for _, t := range tokens {
		if len(t) < tokenMinBytes {
			return nil, ErrTokenTooShort
		}
	}

	prefix := cfg.Prefix
	if prefix == "" {
		prefix = DefaultPrefix
	}
	if !strings.HasPrefix(prefix, "/") || strings.HasSuffix(prefix, "/") {
		return nil, ErrBadPrefix
	}

	if cfg.DownloadCapBytes < 0 || cfg.UploadCapBytes < 0 {
		return nil, ErrBadCap
	}
	downloadCap := cfg.DownloadCapBytes
	if downloadCap == 0 {
		downloadCap = DefaultCapBytes
	}
	if downloadCap > AbsoluteMaxBytes {
		downloadCap = AbsoluteMaxBytes
	}
	uploadCap := cfg.UploadCapBytes
	if uploadCap == 0 {
		uploadCap = DefaultCapBytes
	}
	if uploadCap > AbsoluteMaxBytes {
		uploadCap = AbsoluteMaxBytes
	}

	ratePerIP := cfg.RatePerIP
	if ratePerIP.RPS <= 0 {
		ratePerIP.RPS = 10
	}
	if ratePerIP.Burst <= 0 {
		ratePerIP.Burst = 20
	}
	rateGlobal := cfg.RateGlobal
	if rateGlobal.RPS <= 0 {
		rateGlobal.RPS = 50
	}
	if rateGlobal.Burst <= 0 {
		rateGlobal.Burst = 100
	}

	maxConcurrent := cfg.MaxConcurrent
	if maxConcurrent <= 0 {
		maxConcurrent = 8
	}
	maxTransfers := cfg.MaxConcurrentTransfers
	if maxTransfers <= 0 {
		maxTransfers = 2
	}

	var bd *budget
	if cfg.ByteBudget != nil {
		if cfg.ByteBudget.Bytes <= 0 || cfg.ByteBudget.WindowS <= 0 {
			return nil, ErrBadBudget
		}
		bd = newBudget(cfg.ByteBudget.Bytes, time.Duration(cfg.ByteBudget.WindowS)*time.Second)
	}

	h := &handler{
		prefix:        prefix,
		appName:       cfg.AppName,
		downloadCap:   downloadCap,
		uploadCap:     uploadCap,
		ratePerIP:     ratePerIP,
		rateGlobal:    rateGlobal,
		budgetCfg:     cfg.ByteBudget,
		maxConcurrent: int32(maxConcurrent),
		maxTransfers:  int32(maxTransfers),
		perIP:         newIPLimiter(ratePerIP, perIPTableMaxEntries),
		global:        newLockedBucket(rateGlobal),
		budget:        bd,
		start:         time.Now(),
		enabled: routeSet{
			Health:   true,
			Echo:     !cfg.DisableEcho,
			Download: !cfg.DisableDownload,
			Upload:   !cfg.DisableUpload,
			Info:     !cfg.DisableInfo,
		},
	}
	for _, t := range tokens {
		h.tokenHashes = append(h.tokenHashes, sha256.Sum256([]byte(t)))
	}

	// /health body is precomputed at init except uptime_s (contract §3.1).
	routesJSON, err := json.Marshal(h.enabled)
	if err != nil {
		return nil, err
	}
	pre := `{"contract":"` + ContractVersion + `","status":"ok","sdk":{"lang":"` + sdkLang + `","version":"` + Version + `"}`
	if h.appName != "" {
		appJSON, err := json.Marshal(h.appName)
		if err != nil {
			return nil, err
		}
		pre += `,"app":` + string(appJSON)
	}
	pre += `,"uptime_s":`
	h.healthPre = []byte(pre)
	h.healthPost = []byte(`,"routes":` + string(routesJSON) + `}`)

	return h, nil
}

// Handler builds the LagHound http.Handler. On configuration error (for
// example, no token) it fails closed: the returned handler answers every
// request with a bare 404 and mounts nothing else. Use New to observe the
// error instead.
func Handler(cfg Config) http.Handler {
	h, err := New(cfg)
	if err != nil {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			bare404(w, r)
		})
	}
	return h
}

// Mount registers the LagHound handler on mux under the configured prefix
// (both the exact prefix and everything below it).
func Mount(mux *http.ServeMux, cfg Config) {
	prefix := cfg.Prefix
	if prefix == "" {
		prefix = DefaultPrefix
	}
	h := Handler(cfg)
	mux.Handle(prefix, h)
	mux.Handle(prefix+"/", h)
}

// ServeHTTP implements the contract check order: kill switch → rate and
// concurrency limits → auth → route logic (contract §5).
func (h *handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	started := time.Now()
	// Failure posture (contract §6.7): a panic inside LagHound code becomes a
	// 500 envelope confined to the LagHound route; the host process survives.
	defer func() {
		if rec := recover(); rec != nil {
			h.writeError(w, started, http.StatusInternalServerError, "internal", "internal error", 0)
		}
	}()

	// Kill switch, checked per request: everything is a bare 404, identical
	// to the bad-token response.
	if os.Getenv(envKillSwitch) == "1" {
		bare404(w, r)
		return
	}

	// Resolve the route path. Both mounting styles work: full-path mounts
	// (net/http ServeMux, gorilla PathPrefix) and prefix-stripping mounts
	// (chi Mount, http.StripPrefix).
	rel := r.URL.Path
	if rest, ok := strings.CutPrefix(rel, h.prefix); ok && (rest == "" || rest[0] == '/') {
		rel = rest
	}

	// Limits run before auth (contract §5) so token brute-forcing is
	// throttled like everything else. Limiter rejections on unauthenticated
	// traffic are bare 404s, not 429s, to stay invisible.
	if h.inflight.Add(1) > h.maxConcurrent {
		h.inflight.Add(-1)
		h.limited(w, r, started, 1)
		return
	}
	defer h.inflight.Add(-1)

	now := time.Now()
	globalOK := h.global.allow(now)
	perIPOK := h.perIP.allow(clientIP(r), now)
	if !globalOK || !perIPOK {
		h.limited(w, r, started, 1)
		return
	}

	if !h.authed(r) {
		bare404(w, r)
		return
	}

	// Install a request-scoped mark collector so host code invoked from these
	// routes (or the routes themselves) can record Server-Timing marks.
	r, _ = withBucket(r)

	switch rel {
	case "/health":
		if r.Method != http.MethodGet {
			h.writeError(w, started, http.StatusMethodNotAllowed, "method_not_allowed", "method not allowed", 0)
			return
		}
		h.serveHealth(w, r, started)
	case "/echo":
		if !h.enabled.Echo {
			bare404(w, r)
			return
		}
		if r.Method != http.MethodGet {
			h.writeError(w, started, http.StatusMethodNotAllowed, "method_not_allowed", "method not allowed", 0)
			return
		}
		h.serveEcho(w, r, started)
	case "/download":
		if !h.enabled.Download {
			bare404(w, r)
			return
		}
		if r.Method != http.MethodGet {
			h.writeError(w, started, http.StatusMethodNotAllowed, "method_not_allowed", "method not allowed", 0)
			return
		}
		if !h.acquireTransfer() {
			h.writeError(w, started, http.StatusTooManyRequests, "rate_limited", "rate limit exceeded", 1)
			return
		}
		defer h.transfers.Add(-1)
		h.serveDownload(w, r, started)
	case "/upload":
		if !h.enabled.Upload {
			bare404(w, r)
			return
		}
		if r.Method != http.MethodPost {
			h.writeError(w, started, http.StatusMethodNotAllowed, "method_not_allowed", "method not allowed", 0)
			return
		}
		if !h.acquireTransfer() {
			h.writeError(w, started, http.StatusTooManyRequests, "rate_limited", "rate limit exceeded", 1)
			return
		}
		defer h.transfers.Add(-1)
		h.serveUpload(w, r, started)
	case "/info":
		if !h.enabled.Info {
			bare404(w, r)
			return
		}
		if r.Method != http.MethodGet {
			h.writeError(w, started, http.StatusMethodNotAllowed, "method_not_allowed", "method not allowed", 0)
			return
		}
		h.serveInfo(w, r, started)
	default:
		bare404(w, r)
	}
}

// authed extracts the presented token (X-LagHound-Token wins over
// Authorization: Bearer; the loser is ignored, not compared) and compares it
// in constant time against every configured token hash. Hashing both sides
// with SHA-256 before subtle.ConstantTimeCompare means a length mismatch
// cannot short-circuit observably (contract §5).
func (h *handler) authed(r *http.Request) bool {
	presented := r.Header.Get(headerToken)
	if presented == "" {
		if auth := r.Header.Get("Authorization"); strings.HasPrefix(auth, "Bearer ") {
			presented = auth[len("Bearer "):]
		}
	}
	sum := sha256.Sum256([]byte(presented))
	match := 0
	for i := range h.tokenHashes {
		match |= subtle.ConstantTimeCompare(sum[:], h.tokenHashes[i][:])
	}
	return match == 1
}

func (h *handler) acquireTransfer() bool {
	if h.transfers.Add(1) > h.maxTransfers {
		h.transfers.Add(-1)
		return false
	}
	return true
}

// limited answers a rate/concurrency rejection: 429 + Retry-After for
// authenticated traffic, bare 404 for everyone else (contract §6.2).
func (h *handler) limited(w http.ResponseWriter, r *http.Request, started time.Time, retryAfterS int) {
	if h.authed(r) {
		h.writeError(w, started, http.StatusTooManyRequests, "rate_limited", "rate limit exceeded", retryAfterS)
		return
	}
	bare404(w, r)
}

// bare404 is indistinguishable from net/http's own "route does not exist"
// response: no envelope, no Server-Timing, no LagHound headers (contract §5).
func bare404(w http.ResponseWriter, r *http.Request) {
	http.NotFound(w, r)
}

// clientIP is the socket peer address. X-Forwarded-For is deliberately not
// consulted (contract §6.2).
func clientIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}
