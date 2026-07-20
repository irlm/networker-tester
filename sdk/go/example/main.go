// Command example is a tiny "real service" that embeds the LagHound
// diagnostic endpoint (contract v1) at /laghound, alongside two trivial app
// routes so external probes see realistic Server-Timing.
//
// Run it:
//
//	PORT=8085 LAGHOUND_TOKEN=demo-token-laghound go run .
//
// Then probe it (the SDK also accepts Authorization: Bearer <token>):
//
//	curl -H "X-LagHound-Token: demo-token-laghound" http://localhost:8085/laghound/health
//	curl -H "X-LagHound-Token: demo-token-laghound" "http://localhost:8085/laghound/download?bytes=1048576" -o /dev/null
package main

import (
	"log"
	"net/http"
	"os"
	"time"

	"github.com/irlm/networker-tester/sdk/go/laghound"
)

func main() {
	port := os.Getenv("PORT")
	if port == "" {
		port = "8085"
	}
	token := os.Getenv("LAGHOUND_TOKEN")
	if token == "" {
		token = "demo-token-laghound"
	}

	mux := http.NewServeMux()

	// The host app's own routes. Wrap them with laghound.WithMarks so custom
	// Server-Timing marks (mark-work, ...) surface to the probes.
	mux.Handle("/", laghound.WithMarks(http.HandlerFunc(root)))
	mux.Handle("/work", laghound.WithMarks(http.HandlerFunc(work)))

	// Mount LagHound under /laghound with the shared token. Mount refuses to
	// serve real routes without a token (fail-closed) — here we always have
	// one (env or the demo default).
	laghound.Mount(mux, laghound.Config{
		Token:   token,
		AppName: "go-sample",
	})

	addr := ":" + port
	log.Printf("go sample listening on %s (LagHound at /laghound, app=go-sample)", addr)
	srv := &http.Server{
		Addr:              addr,
		Handler:           mux,
		ReadHeaderTimeout: 10 * time.Second,
	}
	if err := srv.ListenAndServe(); err != nil {
		log.Fatal(err)
	}
}

func root(w http.ResponseWriter, r *http.Request) {
	// ServeMux "/" is a catch-all; only answer the exact root path.
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Write([]byte("go sample ok"))
}

func work(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	// Simulate ~30ms of server-side work so probes see a realistic split.
	select {
	case <-time.After(30 * time.Millisecond):
	case <-r.Context().Done():
	}
	// Record a custom mark; it lands on Server-Timing via WithMarks.
	laghound.MarkSince(r.Context(), "work", start)
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Write([]byte("done"))
}
