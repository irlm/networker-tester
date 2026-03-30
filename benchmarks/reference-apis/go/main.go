// AletheBench Go reference API.
// Pure net/http stdlib — no frameworks, no external dependencies.
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

	srv := &http.Server{
		Addr:      addr,
		Handler:   mux,
		TLSConfig: tlsCfg,
	}

	log.Printf("AletheBench Go reference API listening on %s (TLS)", addr)
	if err := srv.ListenAndServeTLS(certPath, keyPath); err != nil {
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
