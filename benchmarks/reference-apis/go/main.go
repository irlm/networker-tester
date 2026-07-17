// AletheBench Go reference API.
// net/http for HTTP/1.1 + HTTP/2, quic-go for HTTP/3 (QUIC/UDP).
//
// Conforms to the frozen contract in benchmarks/shared/API-SPEC.md (family C).
package main

import (
	"bytes"
	"compress/zlib"
	"crypto/sha256"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"hash/crc32"
	"io"
	"log/slog"
	"math"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/quic-go/quic-go/http3"
)

// ─────────────────────────────────────────────────────────────────────────────
// Shared benchmark dataset (API-SPEC.md §2) — load failure is FATAL
// ─────────────────────────────────────────────────────────────────────────────

// benchFloat serializes float64 the way the Python canonical-JSON validator
// expects: integral values keep a trailing ".0" (json.Marshal would emit
// `39` for float64(39.0), which Python parses as int and re-serializes as
// "39" — breaking the §7 canonical checksums).
type benchFloat float64

func (f benchFloat) MarshalJSON() ([]byte, error) {
	s := strconv.FormatFloat(float64(f), 'g', -1, 64)
	if !strings.ContainsAny(s, ".eE") {
		s += ".0"
	}
	return []byte(s), nil
}

// User mirrors the bench-data.json user schema (§2): no age/active/department.
type User struct {
	ID        int        `json:"id"`
	Name      string     `json:"name"`
	Email     string     `json:"email"`
	Score     benchFloat `json:"score"`
	CreatedAt string     `json:"created_at"`
}

// TimeseriesPoint mirrors the bench-data.json timeseries schema (§2):
// objects {ts, value, category}, not bare floats.
type TimeseriesPoint struct {
	Ts       int     `json:"ts"`
	Value    float64 `json:"value"`
	Category string  `json:"category"`
}

// BenchDataFile mirrors the shared bench-data.json schema (_version 2).
type BenchDataFile struct {
	Version           int               `json:"_version"`
	Users             []User            `json:"users"`
	SearchCorpus      []string          `json:"search_corpus"`
	Timeseries        []TimeseriesPoint `json:"timeseries"`
	TransformInputs   []json.RawMessage `json:"transform_inputs"`
	ExpectedChecksums map[string]string `json:"expected_checksums"`
}

// benchData holds the shared dataset. Always non-nil after loadBenchData
// (the process exits otherwise — no PRNG fallback, audit F2/P0#2).
var benchData *BenchDataFile

// tsValues caches the timeseries values in dataset order so /api/aggregate
// does not re-walk the object list per request (audit F6). The per-request
// work (copy + sort + stats) is the measured workload.
var tsValues []float64

func fatalf(format string, args ...interface{}) {
	fmt.Fprintf(os.Stderr, "FATAL: "+format+"\n", args...)
	os.Exit(1)
}

func parseBenchData(path string) (*BenchDataFile, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var bd BenchDataFile
	if err := json.Unmarshal(data, &bd); err != nil {
		return nil, err
	}
	return &bd, nil
}

// verifyBenchData enforces the §2 schema counts; any mismatch is fatal.
func verifyBenchData(bd *BenchDataFile, path string) {
	if bd.Version != 2 {
		fatalf("bench-data.json at %s: _version=%d, want 2", path, bd.Version)
	}
	if len(bd.Users) != 100 {
		fatalf("bench-data.json at %s: %d users, want 100", path, len(bd.Users))
	}
	if len(bd.SearchCorpus) != 1000 {
		fatalf("bench-data.json at %s: %d search_corpus items, want 1000", path, len(bd.SearchCorpus))
	}
	if len(bd.Timeseries) != 10000 {
		fatalf("bench-data.json at %s: %d timeseries points, want 10000", path, len(bd.Timeseries))
	}
	if len(bd.TransformInputs) != 10 {
		fatalf("bench-data.json at %s: %d transform_inputs, want 10", path, len(bd.TransformInputs))
	}
	if len(bd.ExpectedChecksums) != 4 {
		fatalf("bench-data.json at %s: %d expected_checksums keys, want 4", path, len(bd.ExpectedChecksums))
	}
}

// loadBenchData resolves the dataset per API-SPEC.md §2 and exits non-zero on
// any failure. Resolution order: $BENCH_DATA_PATH, /opt/bench/bench-data.json,
// ../shared/bench-data.json relative to the source/executable directory.
func loadBenchData() {
	if env := os.Getenv("BENCH_DATA_PATH"); env != "" {
		bd, err := parseBenchData(env)
		if err != nil {
			fatalf("BENCH_DATA_PATH=%s could not be loaded: %v (dataset load failure must not fall back)", env, err)
		}
		verifyBenchData(bd, env)
		benchData = bd
		slog.Info("Loaded bench-data.json", "path", env)
		return
	}

	paths := []string{"/opt/bench/bench-data.json"}
	if exe, err := os.Executable(); err == nil {
		paths = append(paths, filepath.Join(filepath.Dir(exe), "..", "shared", "bench-data.json"))
	}
	paths = append(paths, "../shared/bench-data.json")

	for _, p := range paths {
		if _, err := os.Stat(p); err != nil {
			continue
		}
		// First existing path wins; if it fails to parse, that is fatal —
		// silently trying the next path could load a different dataset.
		bd, err := parseBenchData(p)
		if err != nil {
			fatalf("bench-data.json exists at %s but could not be loaded: %v", p, err)
		}
		verifyBenchData(bd, p)
		benchData = bd
		slog.Info("Loaded bench-data.json", "path", p)
		return
	}
	fatalf("bench-data.json not found (tried BENCH_DATA_PATH, /opt/bench/bench-data.json, ../shared/bench-data.json); the shared dataset is required — there is no PRNG fallback")
}

var benchToken = os.Getenv("BENCH_API_TOKEN")

const (
	defaultAddr    = ":8443"
	defaultCertDir = "/opt/bench"
	chunkSize      = 8192          // §5.2: 8 KiB download chunks
	fillByte       = 0x42          // §5.2: fill byte 'B'
	maxDownload    = 2_147_483_648 // §5.2: 2 GiB clamp
)

// healthBody is precomputed once at startup — /health is constant-work (§5.1).
var healthBody = []byte(fmt.Sprintf(
	`{"status":"ok","runtime":"go","version":"%s"}`, runtime.Version()))

// downloadChunk is a shared 8 KiB buffer of 0x42.
var downloadChunk = func() []byte {
	b := make([]byte, chunkSize)
	for i := range b {
		b[i] = fillByte
	}
	return b
}()

func main() {
	// Configure structured logging with LOG_LEVEL env var (debug, info, warn, error).
	logLevel := slog.LevelInfo
	switch strings.ToLower(os.Getenv("LOG_LEVEL")) {
	case "debug":
		logLevel = slog.LevelDebug
	case "warn", "warning":
		logLevel = slog.LevelWarn
	case "error":
		logLevel = slog.LevelError
	}
	if strings.ToLower(os.Getenv("LOG_FORMAT")) == "json" {
		slog.SetDefault(slog.New(
			slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{Level: logLevel}).
				WithAttrs([]slog.Attr{slog.String("service", "go")}),
		))
	} else {
		slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: logLevel})))
	}

	loadBenchData()
	tsValues = make([]float64, len(benchData.Timeseries))
	for i, p := range benchData.Timeseries {
		tsValues[i] = p.Value
	}

	// Worker policy (§3): BENCH_WORKERS maps to GOMAXPROCS, default = cores.
	nproc := runtime.NumCPU()
	workers := nproc
	if w := os.Getenv("BENCH_WORKERS"); w != "" {
		n, err := strconv.Atoi(w)
		if err != nil || n < 1 {
			fatalf("BENCH_WORKERS=%q is not a positive integer", w)
		}
		workers = n
	}
	runtime.GOMAXPROCS(workers)
	slog.Info("Worker policy", "nproc", nproc, "bench_workers", workers, "mechanism", "GOMAXPROCS")

	addr := os.Getenv("LISTEN_ADDR")
	if addr == "" {
		if port := os.Getenv("BENCH_PORT"); port != "" {
			addr = ":" + port
		} else {
			addr = defaultAddr
		}
	}
	certDir := os.Getenv("BENCH_CERT_DIR")
	if certDir == "" {
		certDir = defaultCertDir
	}
	certPath := certDir + "/cert.pem"
	keyPath := certDir + "/key.pem"

	mux := http.NewServeMux()
	mux.HandleFunc("GET /health", handleHealth)
	mux.HandleFunc("GET /download/{size}", handleDownload)
	mux.HandleFunc("POST /upload", handleUpload)
	mux.HandleFunc("GET /api/users", handleAPIUsers)
	mux.HandleFunc("POST /api/transform", handleAPITransform)
	mux.HandleFunc("GET /api/aggregate", handleAPIAggregate)
	mux.HandleFunc("GET /api/search", handleAPISearch)
	mux.HandleFunc("POST /api/upload/process", handleAPIUploadProcess)
	mux.HandleFunc("GET /api/delayed", handleAPIDelayed)
	mux.HandleFunc("GET /api/validate", handleAPIValidate)

	tlsCfg := &tls.Config{
		MinVersion: tls.VersionTLS12,
	}

	// Auth middleware (§1): if BENCH_API_TOKEN is set, every route except
	// /health requires `Authorization: Bearer <token>`.
	authHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/health" && benchToken != "" {
			auth := r.Header.Get("Authorization")
			if !strings.HasPrefix(auth, "Bearer ") || auth[7:] != benchToken {
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(http.StatusUnauthorized)
				w.Write([]byte(`{"error":"unauthorized"}`))
				return
			}
		}
		mux.ServeHTTP(w, r)
	})

	// Wrap handler to advertise HTTP/3 via Alt-Svc header.
	altSvcHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Alt-Svc", fmt.Sprintf(`h3="%s"; ma=86400`, addr))
		authHandler.ServeHTTP(w, r)
	})

	// Check if TLS certs exist — if not, run plain HTTP (application mode behind proxy)
	_, certErr := os.Stat(certPath)
	_, keyErr := os.Stat(keyPath)
	useTLS := certErr == nil && keyErr == nil

	if useTLS {
		// HTTP/3 server (QUIC/UDP) — run in background goroutine.
		h3srv := &http3.Server{Addr: addr, Handler: altSvcHandler}
		go func() {
			slog.Info("HTTP/3 (QUIC) listening", "addr", addr)
			if err := h3srv.ListenAndServeTLS(certPath, keyPath); err != nil {
				slog.Error("HTTP/3 server error", "error", err)
			}
		}()

		// TCP server (HTTP/1.1 + HTTP/2) with TLS.
		tcpSrv := &http.Server{
			Addr:      addr,
			Handler:   altSvcHandler,
			TLSConfig: tlsCfg,
		}
		slog.Info("AletheBench Go reference API listening", "addr", addr, "tls", true, "quic", true)
		if err := tcpSrv.ListenAndServeTLS(certPath, keyPath); err != nil {
			slog.Error("Server failed to start", "error", err)
			os.Exit(1)
		}
	} else {
		// Plain HTTP mode (application mode behind reverse proxy)
		srv := &http.Server{Addr: addr, Handler: authHandler}
		slog.Info("AletheBench Go reference API listening", "addr", addr, "tls", false, "mode", "application")
		if err := srv.ListenAndServe(); err != nil {
			slog.Error("Server failed to start", "error", err)
			os.Exit(1)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Response helpers
// ─────────────────────────────────────────────────────────────────────────────

// setAPIHeaders sets the §1 benchmark headers and returns the start time.
func setAPIHeaders(w http.ResponseWriter) time.Time {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store, no-cache, must-revalidate")
	w.Header().Set("Timing-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Origin", "*")
	return time.Now()
}

// writeServerTiming sets `Server-Timing: app;dur=<ms>` from the start time.
func writeServerTiming(w http.ResponseWriter, start time.Time) {
	dur := float64(time.Since(start).Microseconds()) / 1000.0
	w.Header().Set("Server-Timing", fmt.Sprintf("app;dur=%.1f", dur))
}

// writeJSON encodes v after stamping Server-Timing.
func writeJSON(w http.ResponseWriter, start time.Time, status int, v interface{}) {
	writeServerTiming(w, start)
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(v)
}

// jsonError writes an §1-style `{"error":...}` response (headers included).
func jsonError(w http.ResponseWriter, start time.Time, status int, msg string) {
	writeJSON(w, start, status, map[string]string{"error": msg})
}

// r2 rounds half away from zero to 2 decimals (§5.6).
func r2(x float64) float64 {
	return math.Floor(x*100.0+0.5) / 100.0
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

// GET /health — byte-constant body precomputed at startup (§5.1).
func handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Content-Length", strconv.Itoa(len(healthBody)))
	w.WriteHeader(http.StatusOK)
	w.Write(healthBody)
}

// GET /download/{size} — stream `size` bytes of 0x42 in 8 KiB chunks (§5.2).
func handleDownload(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	sizeStr := r.PathValue("size")
	size, err := strconv.ParseInt(sizeStr, 10, 64)
	if err != nil || size < 0 {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		w.Write([]byte(`{"error":"invalid size"}`))
		return
	}
	if size > maxDownload {
		size = maxDownload
	}

	proc := float64(time.Since(start).Microseconds()) / 1000.0
	w.Header().Set("Content-Type", "application/octet-stream")
	w.Header().Set("Content-Length", strconv.FormatInt(size, 10))
	w.Header().Set("X-Download-Bytes", strconv.FormatInt(size, 10))
	w.Header().Set("Server-Timing", fmt.Sprintf("proc;dur=%.1f", proc))

	remaining := size
	for remaining > 0 {
		chunk := int64(chunkSize)
		if chunk > remaining {
			chunk = remaining
		}
		n, err := w.Write(downloadChunk[:chunk])
		if err != nil {
			return // client disconnected
		}
		remaining -= int64(n)
	}
}

// POST /upload — drain the body without wholesale buffering (§5.3).
func handleUpload(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	n, err := io.Copy(io.Discard, r.Body)
	if err != nil {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusInternalServerError)
		fmt.Fprintf(w, `{"error":"read error: %s"}`, strings.ReplaceAll(err.Error(), `"`, `'`))
		return
	}

	recv := float64(time.Since(start).Microseconds()) / 1000.0
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("X-Networker-Received-Bytes", strconv.FormatInt(n, 10))
	w.Header().Set("Server-Timing", fmt.Sprintf("recv;dur=%.1f", recv))
	if reqID := r.Header.Get("X-Networker-Request-Id"); reqID != "" {
		w.Header().Set("X-Networker-Request-Id", reqID)
	}
	json.NewEncoder(w).Encode(map[string]int64{"received_bytes": n})
}

// GET /api/users?page=N&sort=<field>&order=<asc|desc> (§5.4).
func handleAPIUsers(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	page := 1
	if p, err := strconv.Atoi(r.URL.Query().Get("page")); err == nil && p > 1 {
		page = p
	}
	sortField := r.URL.Query().Get("sort")
	descending := r.URL.Query().Get("order") == "desc"

	// 100-user window of the dataset; page ≥ 2 is empty (dataset has 100 users).
	winStart := (page - 1) * 100
	winEnd := winStart + 100
	if winEnd > len(benchData.Users) {
		winEnd = len(benchData.Users)
	}
	users := make([]User, 0, 100)
	if winStart < len(benchData.Users) {
		users = append(users, benchData.Users[winStart:winEnd]...)
	}

	// Stable sort; string fields compare bytewise, score as float64;
	// unrecognized sort fields fall back to id. `desc` reverses the
	// comparator (ties keep dataset order — sort stays stable).
	cmp := func(a, b *User) int {
		switch sortField {
		case "name":
			return strings.Compare(a.Name, b.Name)
		case "email":
			return strings.Compare(a.Email, b.Email)
		case "score":
			if a.Score < b.Score {
				return -1
			} else if a.Score > b.Score {
				return 1
			}
			return 0
		case "created_at":
			return strings.Compare(a.CreatedAt, b.CreatedAt)
		default:
			return a.ID - b.ID
		}
	}
	sort.SliceStable(users, func(i, j int) bool {
		if descending {
			return cmp(&users[j], &users[i]) < 0
		}
		return cmp(&users[i], &users[j]) < 0
	})

	if len(users) > 20 {
		users = users[:20]
	}
	writeJSON(w, start, http.StatusOK, users)
}

// transformRequest mirrors §5.5: all fields optional.
type transformRequest struct {
	Seed   *json.Number      `json:"seed"`
	Fields []string          `json:"fields"`
	Values []json.RawMessage `json:"values"`
}

// POST /api/transform — SHA-256 each field, reverse values (§5.5).
func handleAPITransform(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	body, err := io.ReadAll(r.Body)
	if err != nil {
		jsonError(w, start, http.StatusBadRequest, "invalid JSON")
		return
	}
	var req transformRequest
	if err := json.Unmarshal(body, &req); err != nil {
		jsonError(w, start, http.StatusBadRequest, "invalid JSON")
		return
	}

	hashed := make([]string, 0, len(req.Fields))
	for _, f := range req.Fields {
		h := sha256.Sum256([]byte(f))
		hashed = append(hashed, fmt.Sprintf("%x", h))
	}

	reversed := make([]json.RawMessage, 0, len(req.Values))
	for i := len(req.Values) - 1; i >= 0; i-- {
		reversed = append(reversed, req.Values[i])
	}

	seed := json.Number("0")
	if req.Seed != nil {
		seed = *req.Seed
	}

	writeJSON(w, start, http.StatusOK, map[string]interface{}{
		"seed":            seed,
		"hashed_fields":   hashed,
		"reversed_values": reversed,
	})
}

type aggregateCategory struct {
	Category string     `json:"category"`
	Count    int        `json:"count"`
	Mean     benchFloat `json:"mean"`
	Min      benchFloat `json:"min"`
	Max      benchFloat `json:"max"`
}

type aggregateResponse struct {
	TotalPoints int                 `json:"total_points"`
	Mean        benchFloat          `json:"mean"`
	P50         benchFloat          `json:"p50"`
	P95         benchFloat          `json:"p95"`
	Max         benchFloat          `json:"max"`
	Categories  []aggregateCategory `json:"categories"`
}

// GET /api/aggregate[?range=start,end] — range is accepted and ignored (§5.6).
func handleAPIAggregate(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	values := make([]float64, len(tsValues))
	copy(values, tsValues)
	sort.Float64s(values)

	n := len(values)
	sum := 0.0
	for _, v := range values {
		sum += v
	}

	chunk := n / 5
	categories := make([]aggregateCategory, 0, 5)
	for i := 0; i < 5; i++ {
		part := values[i*chunk : (i+1)*chunk]
		partSum := 0.0
		for _, v := range part {
			partSum += v
		}
		categories = append(categories, aggregateCategory{
			Category: fmt.Sprintf("q%d", i+1),
			Count:    chunk,
			Mean:     benchFloat(r2(partSum / float64(chunk))),
			Min:      benchFloat(r2(part[0])),
			Max:      benchFloat(r2(part[len(part)-1])),
		})
	}

	writeJSON(w, start, http.StatusOK, aggregateResponse{
		TotalPoints: n,
		Mean:        benchFloat(r2(sum / float64(n))),
		P50:         benchFloat(r2(values[int(float64(n)*0.50)])),
		P95:         benchFloat(r2(values[int(float64(n)*0.95)])),
		Max:         benchFloat(r2(values[n-1])),
		Categories:  categories,
	})
}

type searchResult struct {
	Rank          int    `json:"rank"`
	Item          string `json:"item"`
	MatchPosition int    `json:"match_position"`
}

// GET /api/search?q=<term>&limit=N — case-sensitive regex with literal
// fallback on compile failure (§5.7).
func handleAPISearch(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	q := "test"
	if v := r.URL.Query().Get("q"); v != "" {
		q = v
	}
	limit := 20
	if v := r.URL.Query().Get("limit"); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			limit = n
		}
	}
	if limit > 100 {
		limit = 100
	}
	if limit < 0 {
		limit = 0
	}

	re, reErr := regexp.Compile(q)

	type match struct {
		pos  int
		item string
	}
	matches := make([]match, 0, 64)
	for i := range benchData.SearchCorpus {
		item := benchData.SearchCorpus[i]
		pos := -1
		if reErr == nil {
			if loc := re.FindStringIndex(item); loc != nil {
				pos = loc[0]
			}
		} else {
			pos = strings.Index(item, q)
		}
		if pos >= 0 {
			matches = append(matches, match{pos: pos, item: item})
		}
	}

	// Sort by (position asc, item asc bytewise).
	sort.SliceStable(matches, func(i, j int) bool {
		if matches[i].pos != matches[j].pos {
			return matches[i].pos < matches[j].pos
		}
		return matches[i].item < matches[j].item
	})

	total := len(matches)
	if len(matches) > limit {
		matches = matches[:limit]
	}
	results := make([]searchResult, 0, len(matches))
	for i, m := range matches {
		results = append(results, searchResult{Rank: i + 1, Item: m.item, MatchPosition: m.pos})
	}

	writeJSON(w, start, http.StatusOK, map[string]interface{}{
		"query":         q,
		"total_matches": total,
		"returned":      len(results),
		"results":       results,
	})
}

// POST /api/upload/process — CRC-32 + SHA-256 + zlib level 6 (§5.8).
func handleAPIUploadProcess(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	body, err := io.ReadAll(r.Body)
	if err != nil {
		jsonError(w, start, http.StatusInternalServerError, "read error")
		return
	}

	crc := crc32.ChecksumIEEE(body)
	sha := sha256.Sum256(body)

	// zlib (RFC 1950) at level 6 — NOT raw deflate (§5.8).
	var compressed bytes.Buffer
	zw, _ := zlib.NewWriterLevel(&compressed, 6)
	zw.Write(body)
	zw.Close()

	writeJSON(w, start, http.StatusOK, map[string]interface{}{
		"original_size":   len(body),
		"compressed_size": compressed.Len(),
		"crc32":           fmt.Sprintf("%08x", crc),
		"sha256":          fmt.Sprintf("%x", sha),
	})
}

// GET /api/delayed?ms=N&work=<ignored> — async timer delay (§5.9).
// Goroutine sleep parks the goroutine, not an OS thread — Go's idiomatic
// non-blocking delay.
func handleAPIDelayed(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	ms := 10
	if v := r.URL.Query().Get("ms"); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			ms = n
		}
	}
	if ms < 1 {
		ms = 1
	}
	if ms > 100 {
		ms = 100
	}

	time.Sleep(time.Duration(ms) * time.Millisecond)

	actual := float64(time.Since(start).Microseconds()) / 1000.0
	writeJSON(w, start, http.StatusOK, map[string]interface{}{
		"requested_ms": ms,
		"actual_ms":    math.Round(actual*100) / 100,
	})
}

// GET /api/validate?seed=N — echo the dataset's expected_checksums (§5.10).
func handleAPIValidate(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	seed := 42
	if v := r.URL.Query().Get("seed"); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			seed = n
		}
	}

	writeJSON(w, start, http.StatusOK, map[string]interface{}{
		"seed":      seed,
		"checksums": benchData.ExpectedChecksums,
	})
}
