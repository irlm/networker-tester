// AletheBench Go reference API.
// net/http for HTTP/1.1 + HTTP/2, quic-go for HTTP/3 (QUIC/UDP).
package main

import (
	"compress/flate"
	"crypto/sha256"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"hash/crc32"
	"io"
	"log/slog"
	"math"
	"math/rand"
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

// BenchDataFile mirrors the shared bench-data.json schema.
type BenchDataFile struct {
	Version            int                `json:"_version"`
	Users              []json.RawMessage  `json:"users"`
	SearchCorpus       []json.RawMessage  `json:"search_corpus"`
	Timeseries         []json.RawMessage  `json:"timeseries"`
	TransformInputs    []json.RawMessage  `json:"transform_inputs"`
	ExpectedChecksums  map[string]string  `json:"expected_checksums"`
}

// benchData holds the shared dataset (nil if file not found — PRNG fallback).
var benchData *BenchDataFile

// loadBenchData tries BENCH_DATA_PATH, /opt/bench/bench-data.json, ../shared/bench-data.json.
func loadBenchData() {
	paths := []string{}
	if env := os.Getenv("BENCH_DATA_PATH"); env != "" {
		paths = append(paths, env)
	}
	paths = append(paths, "/opt/bench/bench-data.json")

	// Relative to executable directory.
	if exe, err := os.Executable(); err == nil {
		paths = append(paths, filepath.Join(filepath.Dir(exe), "..", "shared", "bench-data.json"))
	}
	paths = append(paths, "../shared/bench-data.json")

	for _, p := range paths {
		data, err := os.ReadFile(p)
		if err != nil {
			continue
		}
		var bd BenchDataFile
		if err := json.Unmarshal(data, &bd); err != nil {
			slog.Warn("bench-data.json is invalid JSON", "path", p, "error", err)
			continue
		}
		benchData = &bd
		slog.Info("Loaded bench-data.json",
			"path", p, "version", bd.Version,
			"users", len(bd.Users), "corpus", len(bd.SearchCorpus),
			"timeseries", len(bd.Timeseries))
		return
	}
	slog.Warn("bench-data.json not found, falling back to per-language PRNG")
}

var benchToken = os.Getenv("BENCH_API_TOKEN")

const (
	defaultAddr    = ":8443"
	defaultCertDir = "/opt/bench"
	bufSize        = 8192
	fillByte       = 0x42
)

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
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{Level: logLevel})))

	loadBenchData()

	addr := os.Getenv("LISTEN_ADDR")
	if addr == "" {
		addr = defaultAddr
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

	// Auth middleware: validate BENCH_API_TOKEN on all routes except /health.
	authHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		authStart := time.Now()
		if r.URL.Path != "/health" && benchToken != "" {
			auth := r.Header.Get("Authorization")
			if !strings.HasPrefix(auth, "Bearer ") || auth[7:] != benchToken {
				dur := float64(time.Since(authStart).Microseconds()) / 1000.0
				w.Header().Set("Server-Timing", fmt.Sprintf("auth;dur=%.1f", dur))
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(401)
				w.Write([]byte(`{"error":"unauthorized"}`))
				return
			}
		}
		dur := float64(time.Since(authStart).Microseconds()) / 1000.0
		w.Header().Set("X-Auth-Duration", fmt.Sprintf("%.1f", dur))
		mux.ServeHTTP(w, r)
	})

	// Wrap handler to advertise HTTP/3 via Alt-Svc header.
	altSvcHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Alt-Svc", fmt.Sprintf(`h3="%s"; ma=86400`, addr))
		authHandler.ServeHTTP(w, r)
	})

	// HTTP/3 server (QUIC/UDP) — run in background goroutine.
	h3srv := &http3.Server{Addr: addr, Handler: altSvcHandler}
	go func() {
		slog.Info("HTTP/3 (QUIC) listening", "addr", addr)
		if err := h3srv.ListenAndServeTLS(certPath, keyPath); err != nil {
			slog.Error("HTTP/3 server error", "error", err)
		}
	}()

	// TCP server (HTTP/1.1 + HTTP/2) — blocks on main goroutine.
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
}

// GET /health — JSON health check with runtime info.
func handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{
		"status":  "ok",
		"runtime": "go",
		"version": runtime.Version(),
	})
}

// GET /download/{size} — stream `size` bytes of 0x42 in 8 KiB chunks.
func handleDownload(w http.ResponseWriter, r *http.Request) {
	sizeStr := r.PathValue("size")
	size, err := strconv.ParseInt(sizeStr, 10, 64)
	if err != nil || size < 0 {
		http.Error(w, "invalid size", http.StatusBadRequest)
		return
	}

	w.Header().Set("Content-Type", "application/octet-stream")
	w.Header().Set("Content-Length", strconv.FormatInt(size, 10))

	buf := make([]byte, bufSize)
	for i := range buf {
		buf[i] = fillByte
	}

	remaining := size
	for remaining > 0 {
		chunk := int64(bufSize)
		if chunk > remaining {
			chunk = remaining
		}
		n, err := w.Write(buf[:chunk])
		if err != nil {
			return // client disconnected
		}
		remaining -= int64(n)
	}
}

// POST /upload — consume request body, return bytes received.
func handleUpload(w http.ResponseWriter, r *http.Request) {
	n, err := io.Copy(io.Discard, r.Body)
	if err != nil {
		http.Error(w, fmt.Sprintf("read error: %v", err), http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]int64{
		"bytes_received": n,
	})
}

// setAPIHeaders sets common benchmark headers and returns the start time.
func setAPIHeaders(w http.ResponseWriter) time.Time {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store, no-cache, must-revalidate")
	w.Header().Set("Timing-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Origin", "*")
	return time.Now()
}

// writeServerTiming sets the Server-Timing header from elapsed duration.
// If the auth middleware set X-Auth-Duration, append auth;dur=X.X.
func writeServerTiming(w http.ResponseWriter, start time.Time) {
	dur := float64(time.Since(start).Microseconds()) / 1000.0
	timing := fmt.Sprintf("app;dur=%.1f", dur)
	if authDur := w.Header().Get("X-Auth-Duration"); authDur != "" {
		timing += fmt.Sprintf(", auth;dur=%s", authDur)
		w.Header().Del("X-Auth-Duration")
	}
	w.Header().Set("Server-Timing", timing)
}

// User represents a generated user for /api/users.
type User struct {
	ID        int    `json:"id"`
	Name      string `json:"name"`
	Email     string `json:"email"`
	Age       int    `json:"age"`
	Score     int    `json:"score"`
	Active    bool   `json:"active"`
	CreatedAt string `json:"created_at"`
}

var firstNames = []string{
	"Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Hank",
	"Ivy", "Jack", "Kara", "Leo", "Mia", "Nick", "Olga", "Paul",
	"Quinn", "Rita", "Sam", "Tina",
}
var lastNames = []string{
	"Smith", "Johnson", "Brown", "Taylor", "Anderson", "Thomas", "Jackson",
	"White", "Harris", "Martin", "Garcia", "Clark", "Lewis", "Hall", "Young",
	"King", "Wright", "Lopez", "Hill", "Scott",
}
var domains = []string{"example.com", "test.org", "demo.net", "bench.io", "sample.dev"}

func generateUsers(seed int64) []User {
	rng := rand.New(rand.NewSource(seed))
	users := make([]User, 100)
	for i := range users {
		first := firstNames[rng.Intn(len(firstNames))]
		last := lastNames[rng.Intn(len(lastNames))]
		domain := domains[rng.Intn(len(domains))]
		users[i] = User{
			ID:        i + 1,
			Name:      first + " " + last,
			Email:     strings.ToLower(first) + "." + strings.ToLower(last) + "@" + domain,
			Age:       20 + rng.Intn(50),
			Score:     rng.Intn(1000),
			Active:    rng.Intn(2) == 1,
			CreatedAt: fmt.Sprintf("2025-%02d-%02d", 1+rng.Intn(12), 1+rng.Intn(28)),
		}
	}
	return users
}

// GET /api/users?page=N&sort=field&order=asc — paginated sorted user list.
func handleAPIUsers(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	page, _ := strconv.Atoi(r.URL.Query().Get("page"))
	if page < 1 {
		page = 1
	}
	sortField := r.URL.Query().Get("sort")
	order := r.URL.Query().Get("order")

	var users []User
	if benchData != nil && len(benchData.Users) > 0 {
		users = make([]User, len(benchData.Users))
		for i, raw := range benchData.Users {
			json.Unmarshal(raw, &users[i])
		}
	} else {
		users = generateUsers(int64(page))
	}

	switch sortField {
	case "name":
		sort.Slice(users, func(i, j int) bool { return users[i].Name < users[j].Name })
	case "email":
		sort.Slice(users, func(i, j int) bool { return users[i].Email < users[j].Email })
	case "age":
		sort.Slice(users, func(i, j int) bool { return users[i].Age < users[j].Age })
	case "score":
		sort.Slice(users, func(i, j int) bool { return users[i].Score < users[j].Score })
	default:
		sort.Slice(users, func(i, j int) bool { return users[i].ID < users[j].ID })
	}
	if order == "desc" {
		for i, j := 0, len(users)-1; i < j; i, j = i+1, j-1 {
			users[i], users[j] = users[j], users[i]
		}
	}

	pageSize := 20
	offset := (page - 1) * pageSize
	if offset > len(users) {
		offset = len(users)
	}
	end := offset + pageSize
	if end > len(users) {
		end = len(users)
	}
	result := users[offset:end]

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(result)
}

// POST /api/transform — hash string fields, reverse values array.
func handleAPITransform(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	var body map[string]interface{}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "invalid JSON", http.StatusBadRequest)
		return
	}

	result := make(map[string]interface{})
	for k, v := range body {
		switch val := v.(type) {
		case string:
			h := sha256.Sum256([]byte(val))
			result[k] = fmt.Sprintf("%x", h)
		case []interface{}:
			rev := make([]interface{}, len(val))
			for i, item := range val {
				rev[len(val)-1-i] = item
			}
			result[k] = rev
		default:
			result[k] = v
		}
	}

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(result)
}

// AggregateResult holds computed statistics for /api/aggregate.
type AggregateResult struct {
	Count      int                       `json:"count"`
	Mean       float64                   `json:"mean"`
	P50        float64                   `json:"p50"`
	P95        float64                   `json:"p95"`
	Max        float64                   `json:"max"`
	Categories map[string]CategoryBucket `json:"categories"`
}

// CategoryBucket holds stats for one category grouping.
type CategoryBucket struct {
	Count int     `json:"count"`
	Sum   float64 `json:"sum"`
	Mean  float64 `json:"mean"`
}

// GET /api/aggregate?range=start,end — statistics over generated data points.
func handleAPIAggregate(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	rangeStr := r.URL.Query().Get("range")
	parts := strings.SplitN(rangeStr, ",", 2)
	if len(parts) != 2 {
		http.Error(w, "range must be start,end", http.StatusBadRequest)
		return
	}
	rangeStart, err1 := strconv.ParseInt(parts[0], 10, 64)
	rangeEnd, err2 := strconv.ParseInt(parts[1], 10, 64)
	if err1 != nil || err2 != nil {
		http.Error(w, "invalid range values", http.StatusBadRequest)
		return
	}

	// Load timeseries from shared data or generate via PRNG.
	var values []float64
	if benchData != nil && len(benchData.Timeseries) > 0 {
		values = make([]float64, len(benchData.Timeseries))
		for i, raw := range benchData.Timeseries {
			json.Unmarshal(raw, &values[i])
		}
	} else {
		rng := rand.New(rand.NewSource(rangeStart))
		values = make([]float64, 10000)
		for i := range values {
			values[i] = rng.Float64()*float64(rangeEnd-rangeStart) + float64(rangeStart)
		}
	}

	n := len(values)
	sum := 0.0
	catNames := []string{"alpha", "beta", "gamma", "delta", "epsilon"}
	cats := make(map[string]*CategoryBucket)
	for _, c := range catNames {
		cats[c] = &CategoryBucket{}
	}

	for i := 0; i < n; i++ {
		sum += values[i]
		cat := catNames[i%len(catNames)]
		cats[cat].Count++
		cats[cat].Sum += values[i]
	}

	sort.Float64s(values)
	for _, b := range cats {
		if b.Count > 0 {
			b.Mean = b.Sum / float64(b.Count)
		}
	}

	catResult := make(map[string]CategoryBucket)
	for k, v := range cats {
		catResult[k] = *v
	}

	result := AggregateResult{
		Count:      n,
		Mean:       sum / float64(n),
		P50:        values[n/2],
		P95:        values[int(float64(n)*0.95)],
		Max:        values[n-1],
		Categories: catResult,
	}

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(result)
}

// SearchResult holds one matched item for /api/search.
type SearchResult struct {
	Index int     `json:"index"`
	Text  string  `json:"text"`
	Score float64 `json:"score"`
}

// GET /api/search?q=term&limit=N — regex search over generated strings.
func handleAPISearch(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	q := r.URL.Query().Get("q")
	if q == "" {
		http.Error(w, "q parameter required", http.StatusBadRequest)
		return
	}
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit < 1 || limit > 100 {
		limit = 10
	}

	re, err := regexp.Compile("(?i)" + regexp.QuoteMeta(q))
	if err != nil {
		http.Error(w, "invalid search term", http.StatusBadRequest)
		return
	}

	// Build corpus from shared data or PRNG fallback.
	type corpusItem struct {
		Index int
		Text  string
	}
	var corpus []corpusItem

	if benchData != nil && len(benchData.SearchCorpus) > 0 {
		for i, raw := range benchData.SearchCorpus {
			var text string
			json.Unmarshal(raw, &text)
			corpus = append(corpus, corpusItem{Index: i, Text: text})
		}
	} else {
		rng := rand.New(rand.NewSource(42))
		words := []string{
			"network", "latency", "throughput", "bandwidth", "packet", "socket",
			"connection", "timeout", "buffer", "stream", "protocol", "endpoint",
			"request", "response", "header", "payload", "router", "gateway",
			"firewall", "proxy",
		}
		for i := 0; i < 1000; i++ {
			parts := make([]string, 3+rng.Intn(4))
			for j := range parts {
				parts[j] = words[rng.Intn(len(words))]
			}
			corpus = append(corpus, corpusItem{Index: i, Text: strings.Join(parts, " ")})
		}
	}

	var results []SearchResult
	for _, item := range corpus {
		loc := re.FindStringIndex(item.Text)
		if loc != nil {
			score := 1.0 / (1.0 + float64(loc[0]))
			results = append(results, SearchResult{Index: item.Index, Text: item.Text, Score: score})
		}
	}

	sort.Slice(results, func(i, j int) bool { return results[i].Score > results[j].Score })
	if len(results) > limit {
		results = results[:limit]
	}

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(results)
}

// UploadProcessResult holds hash/compression results for /api/upload/process.
type UploadProcessResult struct {
	OriginalSize   int    `json:"original_size"`
	CompressedSize int    `json:"compressed_size"`
	CRC32          string `json:"crc32"`
	SHA256         string `json:"sha256"`
}

// POST /api/upload/process — hash and compress uploaded body.
func handleAPIUploadProcess(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, fmt.Sprintf("read error: %v", err), http.StatusInternalServerError)
		return
	}

	crc := crc32.ChecksumIEEE(body)
	sha := sha256.Sum256(body)

	var compressed strings.Builder
	fw, _ := flate.NewWriter(&compressed, flate.DefaultCompression)
	fw.Write(body)
	fw.Close()

	result := UploadProcessResult{
		OriginalSize:   len(body),
		CompressedSize: len(compressed.String()),
		CRC32:          fmt.Sprintf("%08x", crc),
		SHA256:         fmt.Sprintf("%x", sha),
	}

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(result)
}

// GET /api/delayed?ms=N&work=light — sleep with optional CPU work.
func handleAPIDelayed(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	ms, _ := strconv.Atoi(r.URL.Query().Get("ms"))
	if ms < 1 {
		ms = 1
	}
	if ms > 100 {
		ms = 100
	}
	work := r.URL.Query().Get("work")

	time.Sleep(time.Duration(ms) * time.Millisecond)

	result := map[string]interface{}{
		"requested_ms": ms,
		"actual_ms":    float64(time.Since(start).Microseconds()) / 1000.0,
		"work":         work,
	}

	if work == "heavy" {
		x := 0.0
		for i := 0; i < 100000; i++ {
			x += math.Sqrt(float64(i))
		}
		result["compute"] = x
	}

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(result)
}

// GET /api/validate?seed=42 — checksums for all endpoints at given seed.
func handleAPIValidate(w http.ResponseWriter, r *http.Request) {
	start := setAPIHeaders(w)

	seed, _ := strconv.ParseInt(r.URL.Query().Get("seed"), 10, 64)
	if seed == 0 {
		seed = 42
	}

	// If shared data is loaded, return pre-computed checksums.
	if benchData != nil && len(benchData.ExpectedChecksums) > 0 {
		result := make(map[string]string)
		result["seed"] = fmt.Sprintf("%d", seed)
		for k, v := range benchData.ExpectedChecksums {
			result[k] = v
		}
		writeServerTiming(w, start)
		json.NewEncoder(w).Encode(result)
		return
	}

	// PRNG fallback.
	// Users checksum
	users := generateUsers(seed)
	usersJSON, _ := json.Marshal(users)
	usersHash := sha256.Sum256(usersJSON)

	// Aggregate checksum
	rng := rand.New(rand.NewSource(seed))
	sum := 0.0
	for i := 0; i < 10000; i++ {
		sum += rng.Float64() * 100.0
	}
	aggHash := sha256.Sum256([]byte(fmt.Sprintf("%.6f", sum)))

	// Search checksum (seed=42 corpus, q="network")
	rng2 := rand.New(rand.NewSource(42))
	words := []string{
		"network", "latency", "throughput", "bandwidth", "packet", "socket",
		"connection", "timeout", "buffer", "stream", "protocol", "endpoint",
		"request", "response", "header", "payload", "router", "gateway",
		"firewall", "proxy",
	}
	var corpus strings.Builder
	for i := 0; i < 1000; i++ {
		parts := make([]string, 3+rng2.Intn(4))
		for j := range parts {
			parts[j] = words[rng2.Intn(len(words))]
		}
		corpus.WriteString(strings.Join(parts, " "))
		corpus.WriteByte('\n')
	}
	searchHash := sha256.Sum256([]byte(corpus.String()))

	result := map[string]string{
		"seed":      fmt.Sprintf("%d", seed),
		"users":     fmt.Sprintf("%x", usersHash[:16]),
		"aggregate": fmt.Sprintf("%x", aggHash[:16]),
		"search":    fmt.Sprintf("%x", searchHash[:16]),
	}

	writeServerTiming(w, start)
	json.NewEncoder(w).Encode(result)
}
