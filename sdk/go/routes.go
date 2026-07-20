package laghound

import (
	"encoding/json"
	"errors"
	"io"
	"math"
	"net/http"
	"strconv"
	"time"
)

// echoBody is byte-constant for the lifetime of the process and reflects no
// request input (contract §3.2).
var echoBody = []byte(`{"contract":"v1","ok":true}`)

// fillChunk is the single per-process read-only buffer /download streams
// from; no per-request allocation proportional to N (contract §3.3).
var fillChunk = func() []byte {
	b := make([]byte, chunkSize)
	for i := range b {
		b[i] = 0x42 // 'B', matching networker-endpoint's DOWNLOAD_FILL
	}
	return b
}()

// setCommon stamps the headers every non-bare response carries: Server-Timing
// with app + total (the compat alias deployed testers already parse), any
// host-app marks recorded for this request, Cache-Control, and
// Timing-Allow-Origin (contract §3, §4).
func (h *handler) setCommon(w http.ResponseWriter, r *http.Request, started time.Time) {
	hd := w.Header()
	d := durMS(time.Since(started))
	hd.Set("Server-Timing", "app;dur="+d+", total;dur="+d)
	appendMarksTo(hd, bucketFrom(r.Context()))
	hd.Set("Cache-Control", cacheControl)
	hd.Set("Timing-Allow-Origin", "*")
}

// durMS renders a duration in milliseconds with exactly 3 decimal places
// (contract §4.1 allows at most 3).
func durMS(d time.Duration) string {
	if d < 0 {
		d = 0
	}
	return strconv.FormatFloat(float64(d)/float64(time.Millisecond), 'f', 3, 64)
}

// writeError emits the contract §7 error envelope. Messages are fixed
// strings — request data is never interpolated.
func (h *handler) writeError(w http.ResponseWriter, started time.Time, status int, code, message string, retryAfterS int) {
	hd := w.Header()
	d := durMS(time.Since(started))
	hd.Set("Server-Timing", "app;dur="+d+", total;dur="+d)
	hd.Set("Cache-Control", cacheControl)
	hd.Set("Timing-Allow-Origin", "*")
	hd.Set("Content-Type", "application/json")
	extra := ""
	if retryAfterS > 0 {
		hd.Set("Retry-After", strconv.Itoa(retryAfterS))
		extra = `,"retry_after_ms":` + strconv.Itoa(retryAfterS*1000)
	}
	w.WriteHeader(status)
	io.WriteString(w, `{"contract":"`+ContractVersion+`","error":{"code":"`+code+`","message":"`+message+`"`+extra+`}}`)
}

// serveHealth is O(1): body precomputed at init except uptime_s.
func (h *handler) serveHealth(w http.ResponseWriter, r *http.Request, started time.Time) {
	uptime := strconv.FormatInt(int64(time.Since(h.start)/time.Second), 10)
	h.setCommon(w, r, started)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	w.Write(h.healthPre)
	io.WriteString(w, uptime)
	w.Write(h.healthPost)
}

func (h *handler) serveEcho(w http.ResponseWriter, r *http.Request, started time.Time) {
	if r.ContentLength > echoRequestBodyMax {
		h.writeError(w, started, http.StatusRequestEntityTooLarge, "payload_too_large", "payload too large", 0)
		return
	}
	h.setCommon(w, r, started)
	hd := w.Header()
	hd.Set("Content-Type", "application/json")
	hd.Set("Content-Length", strconv.Itoa(len(echoBody)))
	w.WriteHeader(http.StatusOK)
	w.Write(echoBody)
}

func (h *handler) serveDownload(w http.ResponseWriter, r *http.Request, started time.Time) {
	requested := int64(DefaultCapBytes)
	if vals, ok := r.URL.Query()["bytes"]; ok {
		v, err := strconv.ParseUint(vals[0], 10, 64)
		if err != nil {
			if !errors.Is(err, strconv.ErrRange) {
				// Unparsable/negative → 400; a silent default would make
				// measurements lie. The offending value is never echoed.
				h.writeError(w, started, http.StatusBadRequest, "invalid_param", "invalid parameter", 0)
				return
			}
			v = math.MaxUint64 // syntactically valid but huge → clamp below
		}
		if v > AbsoluteMaxBytes {
			requested = AbsoluteMaxBytes
		} else {
			requested = int64(v)
		}
	}

	// Clamp, not reject (contract §3.3); the actual size is reported via
	// Content-Length and X-LagHound-Bytes so the tester can detect the clamp.
	effective := requested
	if effective > h.downloadCap {
		effective = h.downloadCap
	}

	if h.budget != nil {
		if retryAfterS, ok := h.budget.take(effective, time.Now()); !ok {
			h.writeError(w, started, http.StatusTooManyRequests, "rate_limited", "rate limit exceeded", retryAfterS)
			return
		}
	}

	h.setCommon(w, r, started) // app measures setup only: before the first chunk
	hd := w.Header()
	hd.Set("Content-Type", "application/octet-stream")
	hd.Set("Content-Length", strconv.FormatInt(effective, 10))
	hd.Set(headerBytes, strconv.FormatInt(effective, 10))
	w.WriteHeader(http.StatusOK)
	for remaining := effective; remaining > 0; {
		n := int64(chunkSize)
		if remaining < n {
			n = remaining
		}
		if _, err := w.Write(fillChunk[:n]); err != nil {
			return // client went away; nothing more to do
		}
		remaining -= n
	}
}

func (h *handler) serveUpload(w http.ResponseWriter, r *http.Request, started time.Time) {
	capBytes := h.uploadCap

	// Content-Length over cap → immediate 413 without reading the body
	// (contract §3.4).
	if r.ContentLength > capBytes {
		h.writeError(w, started, http.StatusRequestEntityTooLarge, "payload_too_large", "payload too large", 0)
		return
	}

	if h.budget != nil {
		if retryAfterS, ok := h.budget.take(0, time.Now()); !ok {
			h.writeError(w, started, http.StatusTooManyRequests, "rate_limited", "rate limit exceeded", retryAfterS)
			return
		}
	}

	// Drain and count, never buffer: peak memory is O(chunk), not O(body).
	recvStart := time.Now()
	received, err := io.Copy(io.Discard, io.LimitReader(r.Body, capBytes+1))
	recvDur := time.Since(recvStart)
	if err != nil {
		h.writeError(w, started, http.StatusInternalServerError, "internal", "internal error", 0)
		return
	}
	if received > capBytes {
		// Chunked/unknown length over cap: stop reading and close the
		// connection (contract §3.4).
		w.Header().Set("Connection", "close")
		h.writeError(w, started, http.StatusRequestEntityTooLarge, "payload_too_large", "payload too large", 0)
		return
	}
	if h.budget != nil {
		h.budget.add(received)
	}

	appStart := time.Now()
	body := `{"contract":"` + ContractVersion + `","received_bytes":` + strconv.FormatInt(received, 10) + `}`
	appDur := time.Since(appStart)

	hd := w.Header()
	hd.Set("Server-Timing", "recv;dur="+durMS(recvDur)+", app;dur="+durMS(appDur)+", total;dur="+durMS(recvDur+appDur))
	appendMarksTo(hd, bucketFrom(r.Context()))
	hd.Set("Cache-Control", cacheControl)
	hd.Set("Timing-Allow-Origin", "*")
	hd.Set("Content-Type", "application/json")
	hd.Set(headerBytes, strconv.FormatInt(received, 10))
	w.WriteHeader(http.StatusOK)
	io.WriteString(w, body)
}

// infoBody mirrors contract §3.5 — the SDK's own config only, never the
// token or any derivative of it, never host-app config or environment.
type infoBody struct {
	Contract string     `json:"contract"`
	SDK      infoSDK    `json:"sdk"`
	App      string     `json:"app,omitempty"`
	Prefix   string     `json:"prefix"`
	UptimeS  int64      `json:"uptime_s"`
	TokenSet bool       `json:"token_set"`
	Caps     infoCaps   `json:"caps"`
	Limits   infoLimits `json:"limits"`
	Routes   routeSet   `json:"routes"`
}

type infoSDK struct {
	Lang    string `json:"lang"`
	Version string `json:"version"`
}

type infoCaps struct {
	DownloadBytes    int64 `json:"download_bytes"`
	UploadBytes      int64 `json:"upload_bytes"`
	AbsoluteMaxBytes int64 `json:"absolute_max_bytes"`
}

type infoRate struct {
	RPS   float64 `json:"rps"`
	Burst int     `json:"burst"`
}

type infoBudget struct {
	Bytes   int64 `json:"bytes"`
	WindowS int   `json:"window_s"`
}

type infoLimits struct {
	RatePerIP              infoRate    `json:"rate_per_ip"`
	RateGlobal             infoRate    `json:"rate_global"`
	MaxConcurrent          int         `json:"max_concurrent"`
	MaxConcurrentTransfers int         `json:"max_concurrent_transfers"`
	ByteBudget             *infoBudget `json:"byte_budget"`
}

func (h *handler) serveInfo(w http.ResponseWriter, r *http.Request, started time.Time) {
	var bd *infoBudget
	if h.budgetCfg != nil {
		bd = &infoBudget{Bytes: h.budgetCfg.Bytes, WindowS: h.budgetCfg.WindowS}
	}
	body := infoBody{
		Contract: ContractVersion,
		SDK:      infoSDK{Lang: sdkLang, Version: Version},
		App:      h.appName,
		Prefix:   h.prefix,
		UptimeS:  int64(time.Since(h.start) / time.Second),
		TokenSet: true, // routes only mount with a token; never the value or a derivative
		Caps: infoCaps{
			DownloadBytes:    h.downloadCap,
			UploadBytes:      h.uploadCap,
			AbsoluteMaxBytes: AbsoluteMaxBytes,
		},
		Limits: infoLimits{
			RatePerIP:              infoRate{RPS: h.ratePerIP.RPS, Burst: h.ratePerIP.Burst},
			RateGlobal:             infoRate{RPS: h.rateGlobal.RPS, Burst: h.rateGlobal.Burst},
			MaxConcurrent:          int(h.maxConcurrent),
			MaxConcurrentTransfers: int(h.maxTransfers),
			ByteBudget:             bd,
		},
		Routes: h.enabled,
	}
	buf, err := json.Marshal(body)
	if err != nil {
		h.writeError(w, started, http.StatusInternalServerError, "internal", "internal error", 0)
		return
	}
	h.setCommon(w, r, started)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	w.Write(buf)
}
