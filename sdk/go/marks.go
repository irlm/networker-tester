package laghound

import (
	"context"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Custom Server-Timing marks (contract §4.2, the `mark-<name>` family).
//
// A host-app handler records marks with Mark(r.Context(), "db", elapsed);
// the SDK appends them to that response's Server-Timing header as
// `mark-db;dur=<ms>`. Marks are collected per request in a context-scoped
// bucket, so concurrent requests never mix. Recording is a no-op for a
// request that is not running under a mark collector (WithMarks or a
// LagHound route), for invalid names, and for negative/non-finite durations —
// it is never allowed to raise into the host app.
//
// LagHound's own routes install a collector automatically, so marks recorded
// against the SDK's routes surface too. To surface marks on your own routes,
// wrap them with WithMarks.

const (
	markMaxNameLen = 24
	maxMarks       = 8 // contract §4.1: <= 8 metrics per response
)

// markBucket collects the marks for a single request. It is safe for
// concurrent use so a handler that fans out to goroutines can still record.
type markBucket struct {
	mu    sync.Mutex
	marks []string // pre-rendered "mark-<name>;dur=<ms>" fragments
}

type markCtxKey struct{}

// withBucket returns a request carrying a fresh mark collector, plus the
// bucket itself so the SDK can read the collected marks after the handler
// returns.
func withBucket(r *http.Request) (*http.Request, *markBucket) {
	b := &markBucket{}
	ctx := context.WithValue(r.Context(), markCtxKey{}, b)
	return r.WithContext(ctx), b
}

func bucketFrom(ctx context.Context) *markBucket {
	b, _ := ctx.Value(markCtxKey{}).(*markBucket)
	return b
}

// Mark records a custom Server-Timing mark for the current request. name must
// match [a-z0-9]{1,24} (emitted as mark-<name>); durMS is a non-negative
// duration in milliseconds. It is a no-op outside a LagHound-instrumented
// request and for invalid input — it never panics.
func Mark(ctx context.Context, name string, durMS float64) {
	b := bucketFrom(ctx)
	if b == nil {
		return
	}
	if !validMarkName(name) {
		return
	}
	if durMS < 0 || durMS != durMS { // negative or NaN
		return
	}
	frag := "mark-" + name + ";dur=" + strconv.FormatFloat(durMS, 'f', 3, 64)
	b.mu.Lock()
	if len(b.marks) < maxMarks {
		b.marks = append(b.marks, frag)
	}
	b.mu.Unlock()
}

// MarkSince is a convenience wrapper: MarkSince(ctx, "db", start) records the
// milliseconds elapsed since start.
func MarkSince(ctx context.Context, name string, start time.Time) {
	Mark(ctx, name, float64(time.Since(start))/float64(time.Millisecond))
}

// WithMarks wraps a host-app handler so marks recorded inside it with
// Mark(r.Context(), ...) are appended to the response's Server-Timing header.
// It is optional and independent of the mounted LagHound routes.
func WithMarks(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r2, b := withBucket(r)
		mw := &markResponseWriter{ResponseWriter: w, bucket: b}
		next.ServeHTTP(mw, r2)
	})
}

// markResponseWriter appends collected marks to Server-Timing just before the
// header is flushed.
type markResponseWriter struct {
	http.ResponseWriter
	bucket      *markBucket
	wroteHeader bool
}

func (m *markResponseWriter) appendMarks() {
	if m.wroteHeader {
		return
	}
	m.wroteHeader = true
	appendMarksTo(m.ResponseWriter.Header(), m.bucket)
}

func (m *markResponseWriter) WriteHeader(status int) {
	m.appendMarks()
	m.ResponseWriter.WriteHeader(status)
}

func (m *markResponseWriter) Write(p []byte) (int, error) {
	m.appendMarks()
	return m.ResponseWriter.Write(p)
}

// appendMarksTo folds the bucket's marks into an existing Server-Timing
// header value (or seeds a new one). Respects the <= 8 metric cap.
func appendMarksTo(hd http.Header, b *markBucket) {
	if b == nil {
		return
	}
	b.mu.Lock()
	frags := b.marks
	b.mu.Unlock()
	if len(frags) == 0 {
		return
	}
	existing := hd.Get("Server-Timing")
	room := maxMarks
	if existing != "" {
		room -= strings.Count(existing, ",") + 1
	}
	if room <= 0 {
		return
	}
	if len(frags) > room {
		frags = frags[:room]
	}
	joined := strings.Join(frags, ", ")
	if existing == "" {
		hd.Set("Server-Timing", joined)
	} else {
		hd.Set("Server-Timing", existing+", "+joined)
	}
}

// validMarkName enforces [a-z0-9]{1,24} (contract §4.2, the part after the
// mark- prefix).
func validMarkName(s string) bool {
	if len(s) == 0 || len(s) > markMaxNameLen {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') {
			continue
		}
		return false
	}
	return true
}
