// AletheBench Go reference API.
// net/http for HTTP/1.1 + HTTP/2, quic-go for HTTP/3 (QUIC/UDP).
package main

import (
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"runtime"
	"strconv"

	"github.com/quic-go/quic-go/http3"
)

const (
	defaultAddr    = ":8443"
	defaultCertDir = "/opt/bench"
	bufSize        = 8192
	fillByte       = 0x42
)

func main() {
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

	tlsCfg := &tls.Config{
		MinVersion: tls.VersionTLS12,
	}

	// Wrap handler to advertise HTTP/3 via Alt-Svc header.
	altSvcHandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Alt-Svc", fmt.Sprintf(`h3="%s"; ma=86400`, addr))
		mux.ServeHTTP(w, r)
	})

	// HTTP/3 server (QUIC/UDP) — run in background goroutine.
	h3srv := &http3.Server{Addr: addr, Handler: altSvcHandler}
	go func() {
		log.Printf("HTTP/3 (QUIC) listening on %s", addr)
		if err := h3srv.ListenAndServeTLS(certPath, keyPath); err != nil {
			log.Printf("HTTP/3 server error: %v", err)
		}
	}()

	// TCP server (HTTP/1.1 + HTTP/2) — blocks on main goroutine.
	tcpSrv := &http.Server{
		Addr:      addr,
		Handler:   altSvcHandler,
		TLSConfig: tlsCfg,
	}

	log.Printf("AletheBench Go reference API listening on %s (TLS + QUIC)", addr)
	if err := tcpSrv.ListenAndServeTLS(certPath, keyPath); err != nil {
		log.Fatalf("server error: %v", err)
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
