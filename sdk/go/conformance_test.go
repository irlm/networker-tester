package laghound

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

const testToken = "test-token-0123456789" // >= 16 bytes

// contract mirrors the parts of shared/sdk-contract-v1.json the Go conformance
// suite pins to. Loaded from ../../shared/sdk-contract-v1.json.
type contract struct {
	Contract      string `json:"contract"`
	PrefixDefault string `json:"prefix_default"`
	Auth          struct {
		Header     string   `json:"header"`
		AltHeader  string   `json:"alt_header"`
		AltScheme  string   `json:"alt_scheme"`
		Compare    string   `json:"compare"`
		TokenMin   int      `json:"token_min_bytes"`
		EnvToken   string   `json:"env_token"`
		CheckOrder []string `json:"check_order"`
		OnFailure  struct {
			Status int    `json:"status"`
			Body   string `json:"body"`
			LHHdrs bool   `json:"laghound_headers"`
		} `json:"on_failure"`
	} `json:"auth"`
	KillSwitch struct {
		Env      string `json:"env"`
		Value    string `json:"value"`
		Behavior struct {
			Status int    `json:"status"`
			Body   string `json:"body"`
		} `json:"behavior"`
	} `json:"kill_switch"`
	Caps struct {
		DownloadDefault int64  `json:"download_default_bytes"`
		UploadDefault   int64  `json:"upload_default_bytes"`
		AbsoluteMax     int64  `json:"absolute_max_bytes"`
		EchoBodyMax     int64  `json:"echo_request_body_max_bytes"`
		OverCapDownload string `json:"over_cap_download"`
		OverCapUpload   string `json:"over_cap_upload"`
		ClampHeader     string `json:"clamp_report_header"`
	} `json:"caps"`
	Limits struct {
		RatePerIP struct {
			RPS   float64 `json:"rps"`
			Burst int     `json:"burst"`
		} `json:"rate_per_ip"`
		RateGlobal struct {
			RPS   float64 `json:"rps"`
			Burst int     `json:"burst"`
		} `json:"rate_global"`
		MaxConcurrent int `json:"max_concurrent"`
		MaxTransfers  int `json:"max_concurrent_transfers"`
		PerIPTableMax int `json:"per_ip_table_max_entries"`
	} `json:"limits"`
	ServerTiming struct {
		Header     string `json:"header"`
		MaxMetrics int    `json:"max_metrics"`
		MaxBytes   int    `json:"max_header_bytes"`
	} `json:"server_timing"`
	Routes []struct {
		ID     string `json:"id"`
		Method string `json:"method"`
		Path   string `json:"path"`
	} `json:"routes"`
	ErrorEnvelope struct {
		Codes []struct {
			Code   string `json:"code"`
			Status int    `json:"status"`
		} `json:"codes"`
		BareStatuses []int `json:"bare_statuses"`
	} `json:"error_envelope"`
	SDKLangs []string `json:"sdk_langs"`
}

func loadContract(t *testing.T) contract {
	t.Helper()
	raw, err := os.ReadFile("../../shared/sdk-contract-v1.json")
	if err != nil {
		t.Fatalf("read contract json: %v", err)
	}
	var c contract
	if err := json.Unmarshal(raw, &c); err != nil {
		t.Fatalf("unmarshal contract json: %v", err)
	}
	return c
}

// newServer mounts a default-config handler on an httptest server.
func newServer(t *testing.T, cfg Config) *httptest.Server {
	t.Helper()
	if cfg.Token == "" && cfg.PreviousToken == "" {
		cfg.Token = testToken
	}
	h, err := New(cfg)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	mux := http.NewServeMux()
	prefix := cfg.Prefix
	if prefix == "" {
		prefix = DefaultPrefix
	}
	mux.Handle(prefix, h)
	mux.Handle(prefix+"/", h)
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	return srv
}

func do(t *testing.T, srv *httptest.Server, method, path string, tok string, body io.Reader) *http.Response {
	t.Helper()
	req, err := http.NewRequest(method, srv.URL+path, body)
	if err != nil {
		t.Fatalf("new request: %v", err)
	}
	if tok != "" {
		req.Header.Set(headerToken, tok)
	}
	resp, err := srv.Client().Do(req)
	if err != nil {
		t.Fatalf("do %s %s: %v", method, path, err)
	}
	return resp
}

func readBody(t *testing.T, resp *http.Response) []byte {
	t.Helper()
	defer resp.Body.Close()
	b, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	return b
}

// --- contract sanity: constants match the machine-readable truth ---

func TestContractConstantsMatch(t *testing.T) {
	c := loadContract(t)
	if c.Contract != ContractVersion {
		t.Errorf("contract version: json %q vs const %q", c.Contract, ContractVersion)
	}
	if c.PrefixDefault != DefaultPrefix {
		t.Errorf("prefix default: json %q vs const %q", c.PrefixDefault, DefaultPrefix)
	}
	if c.Auth.Header != headerToken {
		t.Errorf("auth header: json %q vs const %q", c.Auth.Header, headerToken)
	}
	if c.Auth.EnvToken != envToken {
		t.Errorf("env token: json %q vs const %q", c.Auth.EnvToken, envToken)
	}
	if c.Auth.TokenMin != tokenMinBytes {
		t.Errorf("token min: json %d vs const %d", c.Auth.TokenMin, tokenMinBytes)
	}
	if c.KillSwitch.Env != envKillSwitch {
		t.Errorf("kill switch env: json %q vs const %q", c.KillSwitch.Env, envKillSwitch)
	}
	if c.Caps.DownloadDefault != DefaultCapBytes {
		t.Errorf("download default: json %d vs const %d", c.Caps.DownloadDefault, DefaultCapBytes)
	}
	if c.Caps.UploadDefault != DefaultCapBytes {
		t.Errorf("upload default: json %d vs const %d", c.Caps.UploadDefault, DefaultCapBytes)
	}
	if c.Caps.AbsoluteMax != AbsoluteMaxBytes {
		t.Errorf("absolute max: json %d vs const %d", c.Caps.AbsoluteMax, AbsoluteMaxBytes)
	}
	if c.Caps.EchoBodyMax != echoRequestBodyMax {
		t.Errorf("echo body max: json %d vs const %d", c.Caps.EchoBodyMax, echoRequestBodyMax)
	}
	if c.Caps.ClampHeader != headerBytes {
		t.Errorf("clamp header: json %q vs const %q", c.Caps.ClampHeader, headerBytes)
	}
	if c.Limits.PerIPTableMax != perIPTableMaxEntries {
		t.Errorf("per-ip table max: json %d vs const %d", c.Limits.PerIPTableMax, perIPTableMaxEntries)
	}
	// sdk lang "go" must be a recognised lang.
	found := false
	for _, l := range c.SDKLangs {
		if l == sdkLang {
			found = true
		}
	}
	if !found {
		t.Errorf("sdk lang %q not in contract sdk_langs %v", sdkLang, c.SDKLangs)
	}
	// Auth check order must be kill_switch -> rate_limits -> auth -> route.
	want := []string{"kill_switch", "rate_limits", "auth", "route"}
	if strings.Join(c.Auth.CheckOrder, ",") != strings.Join(want, ",") {
		t.Errorf("check order: %v", c.Auth.CheckOrder)
	}
}

// --- routes: every route present, right method, right status ---

func TestRoutesShapePerContract(t *testing.T) {
	c := loadContract(t)
	srv := newServer(t, Config{AppName: "checkout-api"})

	for _, rt := range c.Routes {
		var body io.Reader
		if rt.Method == http.MethodPost {
			body = strings.NewReader("hello")
		}
		resp := do(t, srv, rt.Method, DefaultPrefix+rt.Path, testToken, body)
		b := readBody(t, resp)
		if resp.StatusCode != http.StatusOK {
			t.Fatalf("route %s %s: status %d body %s", rt.Method, rt.Path, resp.StatusCode, b)
		}
		// Every success carries Server-Timing (app) and no-store Cache-Control.
		st := resp.Header.Get("Server-Timing")
		if !strings.Contains(st, "app;dur=") {
			t.Errorf("route %s: missing app in Server-Timing %q", rt.Path, st)
		}
		if !strings.Contains(st, "total;dur=") {
			t.Errorf("route %s: missing total in Server-Timing %q", rt.Path, st)
		}
		if resp.Header.Get("Cache-Control") != "no-store, no-cache, must-revalidate" {
			t.Errorf("route %s: Cache-Control %q", rt.Path, resp.Header.Get("Cache-Control"))
		}
		if resp.Header.Get("Timing-Allow-Origin") != "*" {
			t.Errorf("route %s: Timing-Allow-Origin %q", rt.Path, resp.Header.Get("Timing-Allow-Origin"))
		}
		// JSON routes: contract == v1.
		if rt.ID != "download" {
			var m map[string]any
			if err := json.Unmarshal(b, &m); err != nil {
				t.Fatalf("route %s: body not json: %v", rt.Path, err)
			}
			if m["contract"] != "v1" {
				t.Errorf("route %s: contract %v", rt.Path, m["contract"])
			}
		}
	}
}

func TestHealthShape(t *testing.T) {
	srv := newServer(t, Config{AppName: "checkout-api"})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/health", testToken, nil)
	b := readBody(t, resp)
	var m struct {
		Contract string          `json:"contract"`
		Status   string          `json:"status"`
		SDK      map[string]any  `json:"sdk"`
		App      string          `json:"app"`
		UptimeS  int64           `json:"uptime_s"`
		Routes   map[string]bool `json:"routes"`
	}
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatalf("health json: %v (%s)", err, b)
	}
	if m.Status != "ok" || m.Contract != "v1" {
		t.Errorf("health status/contract: %+v", m)
	}
	if m.SDK["lang"] != "go" || m.SDK["version"] != Version {
		t.Errorf("health sdk: %+v", m.SDK)
	}
	if m.App != "checkout-api" {
		t.Errorf("health app: %q", m.App)
	}
	for _, r := range []string{"health", "echo", "download", "upload", "info"} {
		if !m.Routes[r] {
			t.Errorf("health routes missing %q", r)
		}
	}
}

func TestEchoFixedBody(t *testing.T) {
	srv := newServer(t, Config{})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/echo", testToken, nil)
	b := readBody(t, resp)
	if string(b) != `{"contract":"v1","ok":true}` {
		t.Errorf("echo body %q", b)
	}
	if len(b) >= 1024 {
		t.Errorf("echo body must be < 1 KiB, got %d", len(b))
	}
}

// --- download: clamp, absolute max, invalid param, fill byte ---

func TestDownloadDefaultSize(t *testing.T) {
	srv := newServer(t, Config{})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/download", testToken, nil)
	b := readBody(t, resp)
	if int64(len(b)) != DefaultCapBytes {
		t.Errorf("default download size %d want %d", len(b), DefaultCapBytes)
	}
	if resp.Header.Get(headerBytes) != strconv.Itoa(DefaultCapBytes) {
		t.Errorf("X-LagHound-Bytes %q", resp.Header.Get(headerBytes))
	}
	if resp.Header.Get("Content-Type") != "application/octet-stream" {
		t.Errorf("download content-type %q", resp.Header.Get("Content-Type"))
	}
	for i, by := range b {
		if by != 0x42 {
			t.Fatalf("fill byte at %d = %#x want 0x42", i, by)
		}
	}
}

func TestDownloadClampToConfigCap(t *testing.T) {
	srv := newServer(t, Config{DownloadCapBytes: 1024})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/download?bytes=1000000", testToken, nil)
	b := readBody(t, resp)
	if int64(len(b)) != 1024 {
		t.Errorf("clamped size %d want 1024", len(b))
	}
	if resp.Header.Get(headerBytes) != "1024" {
		t.Errorf("clamp header %q want 1024", resp.Header.Get(headerBytes))
	}
	if resp.Header.Get("Content-Length") != "1024" {
		t.Errorf("content-length %q", resp.Header.Get("Content-Length"))
	}
}

func TestDownloadClampToAbsoluteMax(t *testing.T) {
	// Config asks for more than the absolute max; New must clamp the cap.
	srv := newServer(t, Config{DownloadCapBytes: AbsoluteMaxBytes * 4})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/download?bytes=999999999999", testToken, nil)
	b := readBody(t, resp)
	if int64(len(b)) != AbsoluteMaxBytes {
		t.Errorf("abs-max size %d want %d", len(b), AbsoluteMaxBytes)
	}
	if resp.Header.Get(headerBytes) != strconv.Itoa(AbsoluteMaxBytes) {
		t.Errorf("abs-max header %q", resp.Header.Get(headerBytes))
	}
}

func TestDownloadInvalidParam(t *testing.T) {
	srv := newServer(t, Config{})
	for _, bad := range []string{"abc", "-5", "1.5", ""} {
		resp := do(t, srv, http.MethodGet, DefaultPrefix+"/download?bytes="+bad, testToken, nil)
		b := readBody(t, resp)
		if resp.StatusCode != http.StatusBadRequest {
			t.Errorf("bytes=%q status %d want 400 (%s)", bad, resp.StatusCode, b)
			continue
		}
		var env struct {
			Contract string `json:"contract"`
			Error    struct {
				Code    string `json:"code"`
				Message string `json:"message"`
			} `json:"error"`
		}
		if err := json.Unmarshal(b, &env); err != nil {
			t.Errorf("bytes=%q envelope json: %v", bad, err)
			continue
		}
		if env.Error.Code != "invalid_param" {
			t.Errorf("bytes=%q code %q", bad, env.Error.Code)
		}
		if strings.Contains(env.Error.Message, bad) && bad != "" {
			t.Errorf("bytes=%q: error message echoes the offending value", bad)
		}
	}
}

// --- upload: over-cap without reading, chunked over-cap, success ---

func TestUploadContentLengthOverCap(t *testing.T) {
	srv := newServer(t, Config{UploadCapBytes: 1024})
	// Content-Length known and over cap → 413 without reading the body.
	big := bytes.NewReader(make([]byte, 4096))
	resp := do(t, srv, http.MethodPost, DefaultPrefix+"/upload", testToken, big)
	b := readBody(t, resp)
	if resp.StatusCode != http.StatusRequestEntityTooLarge {
		t.Fatalf("over-cap upload status %d want 413 (%s)", resp.StatusCode, b)
	}
	var env struct {
		Error struct {
			Code string `json:"code"`
		} `json:"error"`
	}
	json.Unmarshal(b, &env)
	if env.Error.Code != "payload_too_large" {
		t.Errorf("over-cap upload code %q", env.Error.Code)
	}
}

func TestUploadChunkedOverCap(t *testing.T) {
	srv := newServer(t, Config{UploadCapBytes: 1024})
	// Force chunked (unknown length) by wrapping in a reader with no Len.
	req, _ := http.NewRequest(http.MethodPost, srv.URL+DefaultPrefix+"/upload", &unknownLenReader{n: 8192})
	req.Header.Set(headerToken, testToken)
	req.ContentLength = -1 // unknown → chunked
	resp, err := srv.Client().Do(req)
	if err != nil {
		t.Fatalf("chunked upload: %v", err)
	}
	b := readBody(t, resp)
	if resp.StatusCode != http.StatusRequestEntityTooLarge {
		t.Fatalf("chunked over-cap status %d want 413 (%s)", resp.StatusCode, b)
	}
}

func TestUploadSuccess(t *testing.T) {
	srv := newServer(t, Config{})
	payload := make([]byte, 2048)
	resp := do(t, srv, http.MethodPost, DefaultPrefix+"/upload", testToken, bytes.NewReader(payload))
	b := readBody(t, resp)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("upload status %d (%s)", resp.StatusCode, b)
	}
	var m struct {
		Contract      string `json:"contract"`
		ReceivedBytes int64  `json:"received_bytes"`
	}
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatalf("upload json: %v (%s)", err, b)
	}
	if m.ReceivedBytes != 2048 || m.Contract != "v1" {
		t.Errorf("upload result %+v", m)
	}
	if resp.Header.Get(headerBytes) != "2048" {
		t.Errorf("upload X-LagHound-Bytes %q", resp.Header.Get(headerBytes))
	}
	st := resp.Header.Get("Server-Timing")
	if !strings.Contains(st, "recv;dur=") || !strings.Contains(st, "app;dur=") {
		t.Errorf("upload Server-Timing missing recv/app: %q", st)
	}
}

// --- auth / invisibility ---

func TestBadTokenBare404OnEveryRoute(t *testing.T) {
	srv := newServer(t, Config{})
	for _, p := range []string{"/health", "/echo", "/download", "/info"} {
		for _, tok := range []string{"", "wrong-token-value-here"} {
			resp := do(t, srv, http.MethodGet, DefaultPrefix+p, tok, nil)
			assertBare404(t, resp, p+" tok="+tok)
		}
	}
	// upload too
	resp := do(t, srv, http.MethodPost, DefaultPrefix+"/upload", "wrong-token-value-here", strings.NewReader("x"))
	assertBare404(t, resp, "/upload bad token")
}

func TestBearerTokenAccepted(t *testing.T) {
	srv := newServer(t, Config{})
	req, _ := http.NewRequest(http.MethodGet, srv.URL+DefaultPrefix+"/echo", nil)
	req.Header.Set("Authorization", "Bearer "+testToken)
	resp, err := srv.Client().Do(req)
	if err != nil {
		t.Fatalf("bearer: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("bearer echo status %d want 200", resp.StatusCode)
	}
	readBody(t, resp)
}

func TestXTokenWinsOverBearer(t *testing.T) {
	srv := newServer(t, Config{})
	// Correct X-LagHound-Token, wrong Bearer → X wins → 200.
	req, _ := http.NewRequest(http.MethodGet, srv.URL+DefaultPrefix+"/echo", nil)
	req.Header.Set(headerToken, testToken)
	req.Header.Set("Authorization", "Bearer wrong")
	resp, err := srv.Client().Do(req)
	if err != nil {
		t.Fatalf("x-wins: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("x-wins status %d want 200", resp.StatusCode)
	}
	readBody(t, resp)

	// Wrong X-LagHound-Token, correct Bearer → X (wrong) is used, Bearer
	// ignored → bare 404.
	req2, _ := http.NewRequest(http.MethodGet, srv.URL+DefaultPrefix+"/echo", nil)
	req2.Header.Set(headerToken, "wrong-token-value-here")
	req2.Header.Set("Authorization", "Bearer "+testToken)
	resp2, err := srv.Client().Do(req2)
	if err != nil {
		t.Fatalf("x-wins2: %v", err)
	}
	assertBare404(t, resp2, "wrong X + right Bearer")
}

func TestTokenRotationPreviousAccepted(t *testing.T) {
	srv := newServer(t, Config{Token: "current-token-abcdef", PreviousToken: "previous-token-abcdef"})
	for _, tok := range []string{"current-token-abcdef", "previous-token-abcdef"} {
		resp := do(t, srv, http.MethodGet, DefaultPrefix+"/echo", tok, nil)
		if resp.StatusCode != http.StatusOK {
			t.Errorf("rotation token %q status %d want 200", tok, resp.StatusCode)
		}
		readBody(t, resp)
	}
}

func assertBare404(t *testing.T, resp *http.Response, ctx string) {
	t.Helper()
	if resp.StatusCode != http.StatusNotFound {
		t.Errorf("%s: status %d want 404", ctx, resp.StatusCode)
	}
	if resp.Header.Get("Server-Timing") != "" {
		t.Errorf("%s: bare 404 carries Server-Timing", ctx)
	}
	if resp.Header.Get("Cache-Control") == "no-store, no-cache, must-revalidate" {
		t.Errorf("%s: bare 404 carries LagHound Cache-Control", ctx)
	}
	if resp.Header.Get(headerBytes) != "" {
		t.Errorf("%s: bare 404 carries X-LagHound-Bytes", ctx)
	}
	readBody(t, resp)
}

// --- kill switch ---

func TestKillSwitch(t *testing.T) {
	os.Setenv(envKillSwitch, "1")
	t.Cleanup(func() { os.Unsetenv(envKillSwitch) })
	srv := newServer(t, Config{})
	for _, p := range []string{"/health", "/echo", "/download", "/info"} {
		resp := do(t, srv, http.MethodGet, DefaultPrefix+p, testToken, nil)
		assertBare404(t, resp, "kill-switch "+p)
	}
}

// --- method not allowed (authed) ---

func TestMethodNotAllowed(t *testing.T) {
	srv := newServer(t, Config{})
	resp := do(t, srv, http.MethodPost, DefaultPrefix+"/echo", testToken, strings.NewReader(""))
	b := readBody(t, resp)
	if resp.StatusCode != http.StatusMethodNotAllowed {
		t.Fatalf("POST /echo status %d want 405 (%s)", resp.StatusCode, b)
	}
	var env struct {
		Error struct {
			Code string `json:"code"`
		} `json:"error"`
	}
	json.Unmarshal(b, &env)
	if env.Error.Code != "method_not_allowed" {
		t.Errorf("405 code %q", env.Error.Code)
	}
}

// --- info: no secrets ---

func TestInfoNoSecrets(t *testing.T) {
	tok := "super-secret-token-value-xyz"
	srv := newServer(t, Config{Token: tok, AppName: "checkout-api"})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/info", tok, nil)
	b := readBody(t, resp)
	if bytes.Contains(b, []byte(tok)) {
		t.Fatalf("/info body leaks the token")
	}
	var m struct {
		TokenSet bool `json:"token_set"`
		Caps     struct {
			AbsoluteMaxBytes int64 `json:"absolute_max_bytes"`
		} `json:"caps"`
		Limits struct {
			MaxConcurrent int `json:"max_concurrent"`
		} `json:"limits"`
	}
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatalf("info json: %v (%s)", err, b)
	}
	if !m.TokenSet {
		t.Errorf("info token_set should be true")
	}
	if m.Caps.AbsoluteMaxBytes != AbsoluteMaxBytes {
		t.Errorf("info absolute max %d", m.Caps.AbsoluteMaxBytes)
	}
	if m.Limits.MaxConcurrent != 8 {
		t.Errorf("info max_concurrent %d want 8", m.Limits.MaxConcurrent)
	}
}

// --- fail-closed: no token ---

func TestFailClosedNoToken(t *testing.T) {
	os.Unsetenv(envToken)
	_, err := New(Config{})
	if err != ErrNoToken {
		t.Fatalf("New without token: err %v want ErrNoToken", err)
	}
	// Handler must still produce a working handler that bare-404s everything.
	h := Handler(Config{})
	srv := httptest.NewServer(h)
	t.Cleanup(srv.Close)
	resp, err := srv.Client().Get(srv.URL + "/laghound/health")
	if err != nil {
		t.Fatalf("failclosed get: %v", err)
	}
	assertBare404(t, resp, "fail-closed handler")
}

func TestTokenTooShort(t *testing.T) {
	_, err := New(Config{Token: "short"})
	if err != ErrTokenTooShort {
		t.Fatalf("short token: err %v want ErrTokenTooShort", err)
	}
}

func TestEnvTokenFallback(t *testing.T) {
	os.Setenv(envToken, testToken)
	t.Cleanup(func() { os.Unsetenv(envToken) })
	h, err := New(Config{})
	if err != nil {
		t.Fatalf("env-token New: %v", err)
	}
	srv := httptest.NewServer(mountOn(h))
	t.Cleanup(srv.Close)
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/echo", testToken, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("env-token echo status %d", resp.StatusCode)
	}
	readBody(t, resp)
}

func mountOn(h http.Handler) *http.ServeMux {
	mux := http.NewServeMux()
	mux.Handle(DefaultPrefix, h)
	mux.Handle(DefaultPrefix+"/", h)
	return mux
}

// --- bad prefix / bad cap validation ---

func TestBadConfigValidation(t *testing.T) {
	if _, err := New(Config{Token: testToken, Prefix: "laghound"}); err != ErrBadPrefix {
		t.Errorf("no-leading-slash prefix: %v", err)
	}
	if _, err := New(Config{Token: testToken, Prefix: "/laghound/"}); err != ErrBadPrefix {
		t.Errorf("trailing-slash prefix: %v", err)
	}
	if _, err := New(Config{Token: testToken, DownloadCapBytes: -1}); err != ErrBadCap {
		t.Errorf("negative cap: %v", err)
	}
	if _, err := New(Config{Token: testToken, ByteBudget: &ByteBudget{Bytes: 0, WindowS: 10}}); err != ErrBadBudget {
		t.Errorf("bad budget: %v", err)
	}
}

// --- disabled route → bare 404, health map reflects it ---

func TestDisabledRoute(t *testing.T) {
	srv := newServer(t, Config{DisableUpload: true})
	resp := do(t, srv, http.MethodPost, DefaultPrefix+"/upload", testToken, strings.NewReader("x"))
	assertBare404(t, resp, "disabled upload")

	h := do(t, srv, http.MethodGet, DefaultPrefix+"/health", testToken, nil)
	b := readBody(t, h)
	var m struct {
		Routes map[string]bool `json:"routes"`
	}
	json.Unmarshal(b, &m)
	if m.Routes["upload"] {
		t.Errorf("health should report upload disabled")
	}
	if !m.Routes["health"] {
		t.Errorf("health must always be true")
	}
}

// --- concurrency transfer cap → 429 ---

func TestConcurrentTransferCap(t *testing.T) {
	// Cap transfers at 1. Hold one download open, fire a second → 429.
	release := make(chan struct{})
	blocker := &blockingReader{release: release}

	cfg := Config{Token: testToken, MaxConcurrentTransfers: 1, DownloadCapBytes: 64 << 10}
	h, err := New(cfg)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	srv := httptest.NewServer(mountOn(h))
	t.Cleanup(srv.Close)

	// Open a slow download that keeps a transfer slot busy: use an upload with
	// a body that blocks mid-drain.
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		req, _ := http.NewRequest(http.MethodPost, srv.URL+DefaultPrefix+"/upload", blocker)
		req.Header.Set(headerToken, testToken)
		req.ContentLength = -1
		resp, err := srv.Client().Do(req)
		if err == nil {
			io.Copy(io.Discard, resp.Body)
			resp.Body.Close()
		}
	}()

	// Wait until the blocking upload is actually draining (slot held).
	if !waitFor(func() bool { return blocker.started() }, 2*time.Second) {
		close(release)
		wg.Wait()
		t.Skip("blocking upload never started; environment too slow")
	}

	// Second transfer should be rejected with 429.
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/download?bytes=1024", testToken, nil)
	b := readBody(t, resp)
	if resp.StatusCode != http.StatusTooManyRequests {
		t.Errorf("second transfer status %d want 429 (%s)", resp.StatusCode, b)
	}
	if resp.Header.Get("Retry-After") == "" {
		t.Errorf("429 missing Retry-After")
	}

	close(release)
	wg.Wait()
}

// --- byte budget → 429 + Retry-After ---

func TestByteBudget(t *testing.T) {
	// Budget of 1 KiB per 60s window; first 4 MiB download exhausts it, the
	// second is rejected.
	srv := newServer(t, Config{ByteBudget: &ByteBudget{Bytes: 1024, WindowS: 60}, DownloadCapBytes: 4096})
	resp1 := do(t, srv, http.MethodGet, DefaultPrefix+"/download?bytes=4096", testToken, nil)
	readBody(t, resp1)
	if resp1.StatusCode != http.StatusOK {
		t.Fatalf("first budget download status %d", resp1.StatusCode)
	}
	resp2 := do(t, srv, http.MethodGet, DefaultPrefix+"/download?bytes=4096", testToken, nil)
	b := readBody(t, resp2)
	if resp2.StatusCode != http.StatusTooManyRequests {
		t.Fatalf("second budget download status %d want 429 (%s)", resp2.StatusCode, b)
	}
	if resp2.Header.Get("Retry-After") == "" {
		t.Errorf("budget 429 missing Retry-After")
	}
	var env struct {
		Error struct {
			Code         string `json:"code"`
			RetryAfterMS int64  `json:"retry_after_ms"`
		} `json:"error"`
	}
	json.Unmarshal(b, &env)
	if env.Error.Code != "rate_limited" {
		t.Errorf("budget code %q", env.Error.Code)
	}
	if env.Error.RetryAfterMS <= 0 {
		t.Errorf("budget retry_after_ms %d", env.Error.RetryAfterMS)
	}
}

// --- per-IP rate limit: unauth stays bare 404, authed becomes 429 ---

func TestRateLimitUnauthBare404(t *testing.T) {
	// Tiny burst so we exhaust it fast. Unauthenticated floods stay bare 404.
	srv := newServer(t, Config{RatePerIP: Rate{RPS: 1, Burst: 1}, RateGlobal: Rate{RPS: 1, Burst: 1}})
	sawBare := false
	for i := 0; i < 10; i++ {
		resp := do(t, srv, http.MethodGet, DefaultPrefix+"/echo", "wrong-token-value-here", nil)
		readBody(t, resp)
		if resp.StatusCode == http.StatusNotFound && resp.Header.Get("Server-Timing") == "" {
			sawBare = true
		}
		if resp.StatusCode == http.StatusTooManyRequests {
			t.Fatalf("unauthenticated throttle returned 429 (must be bare 404)")
		}
	}
	if !sawBare {
		t.Errorf("expected bare 404s for unauthenticated flood")
	}
}

func TestRateLimitAuthed429(t *testing.T) {
	srv := newServer(t, Config{RatePerIP: Rate{RPS: 1, Burst: 1}, RateGlobal: Rate{RPS: 1000, Burst: 1000}})
	saw429 := false
	for i := 0; i < 10; i++ {
		resp := do(t, srv, http.MethodGet, DefaultPrefix+"/echo", testToken, nil)
		readBody(t, resp)
		if resp.StatusCode == http.StatusTooManyRequests {
			saw429 = true
			if resp.Header.Get("Retry-After") == "" {
				t.Errorf("authed 429 missing Retry-After")
			}
		}
	}
	if !saw429 {
		t.Errorf("expected a 429 for authenticated flood")
	}
}

// --- custom marks surface on Server-Timing ---

func TestMarksViaWithMarks(t *testing.T) {
	inner := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		Mark(r.Context(), "db", 41.9)
		Mark(r.Context(), "cache", 2.5)
		Mark(r.Context(), "BAD NAME", 1) // invalid → ignored
		Mark(r.Context(), "neg", -1)     // negative → ignored
		w.Header().Set("Server-Timing", "app;dur=57.1")
		w.WriteHeader(http.StatusOK)
		io.WriteString(w, "ok")
	})
	srv := httptest.NewServer(WithMarks(inner))
	t.Cleanup(srv.Close)
	resp, err := srv.Client().Get(srv.URL + "/")
	if err != nil {
		t.Fatalf("marks get: %v", err)
	}
	readBody(t, resp)
	st := resp.Header.Get("Server-Timing")
	if !strings.Contains(st, "mark-db;dur=41.900") {
		t.Errorf("missing mark-db in %q", st)
	}
	if !strings.Contains(st, "mark-cache;dur=2.500") {
		t.Errorf("missing mark-cache in %q", st)
	}
	if strings.Contains(st, "BAD") || strings.Contains(st, "mark-neg") {
		t.Errorf("invalid mark leaked into %q", st)
	}
}

func TestMarkNoOpOutsideRequest(t *testing.T) {
	// Must never panic when there is no collector.
	Mark(context.Background(), "db", 1.0)
	MarkSince(context.Background(), "db", time.Now())
}

// --- Server-Timing budget: <= 8 metrics, <= 512 bytes ---

func TestServerTimingBounds(t *testing.T) {
	c := loadContract(t)
	srv := newServer(t, Config{})
	resp := do(t, srv, http.MethodGet, DefaultPrefix+"/echo", testToken, nil)
	readBody(t, resp)
	st := resp.Header.Get("Server-Timing")
	if len(st) > c.ServerTiming.MaxBytes {
		t.Errorf("Server-Timing %d bytes > max %d", len(st), c.ServerTiming.MaxBytes)
	}
	metrics := strings.Count(st, ";dur=")
	if metrics > c.ServerTiming.MaxMetrics {
		t.Errorf("Server-Timing %d metrics > max %d", metrics, c.ServerTiming.MaxMetrics)
	}
}

// --- Mount helper ---

func TestMountHelper(t *testing.T) {
	mux := http.NewServeMux()
	Mount(mux, Config{Token: testToken, Prefix: "/lh"})
	srv := httptest.NewServer(mux)
	t.Cleanup(srv.Close)
	resp := do(t, srv, http.MethodGet, "/lh/health", testToken, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("mount /lh/health status %d", resp.StatusCode)
	}
	readBody(t, resp)
}

// --- test helpers ---

// unknownLenReader produces n bytes with no known length (forces chunked).
type unknownLenReader struct {
	n    int
	done int
}

func (u *unknownLenReader) Read(p []byte) (int, error) {
	if u.done >= u.n {
		return 0, io.EOF
	}
	rem := u.n - u.done
	if rem > len(p) {
		rem = len(p)
	}
	for i := 0; i < rem; i++ {
		p[i] = 'x'
	}
	u.done += rem
	return rem, nil
}

// blockingReader emits a little data, then blocks on release so the upload
// keeps its transfer slot until the test lets go.
type blockingReader struct {
	release chan struct{}
	mu      sync.Mutex
	begun   bool
	blocked bool
}

func (b *blockingReader) started() bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.blocked
}

func (b *blockingReader) Read(p []byte) (int, error) {
	b.mu.Lock()
	if !b.begun {
		b.begun = true
		b.mu.Unlock()
		p[0] = 'x'
		return 1, nil
	}
	b.blocked = true
	b.mu.Unlock()
	<-b.release
	return 0, io.EOF
}

func waitFor(cond func() bool, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return true
		}
		time.Sleep(2 * time.Millisecond)
	}
	return cond()
}
