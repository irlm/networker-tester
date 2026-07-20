package laghound

import (
	"container/list"
	"sync"
	"time"
)

// bucket is a token bucket. It is not safe for concurrent use on its own;
// callers hold a lock (lockedBucket, ipLimiter).
type bucket struct {
	tokens float64
	last   time.Time
}

func newBucket(r Rate, now time.Time) bucket {
	return bucket{tokens: float64(r.Burst), last: now}
}

func (b *bucket) allow(r Rate, now time.Time) bool {
	elapsed := now.Sub(b.last).Seconds()
	if elapsed > 0 {
		b.tokens += elapsed * r.RPS
		if b.tokens > float64(r.Burst) {
			b.tokens = float64(r.Burst)
		}
		b.last = now
	}
	if b.tokens >= 1 {
		b.tokens--
		return true
	}
	return false
}

// lockedBucket is the process-wide rate limiter.
type lockedBucket struct {
	mu   sync.Mutex
	rate Rate
	b    bucket
}

func newLockedBucket(r Rate) *lockedBucket {
	return &lockedBucket{rate: r, b: newBucket(r, time.Now())}
}

func (l *lockedBucket) allow(now time.Time) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.allow(l.rate, now)
}

// ipLimiter keeps one token bucket per client IP in an LRU table capped at
// maxEntries (default 10 000) so an address-spraying attacker cannot grow
// memory (contract §6.2).
type ipLimiter struct {
	mu         sync.Mutex
	rate       Rate
	maxEntries int
	entries    map[string]*list.Element
	lru        *list.List // front = most recently used
}

type ipEntry struct {
	ip string
	b  bucket
}

func newIPLimiter(r Rate, maxEntries int) *ipLimiter {
	return &ipLimiter{
		rate:       r,
		maxEntries: maxEntries,
		entries:    make(map[string]*list.Element),
		lru:        list.New(),
	}
}

func (l *ipLimiter) allow(ip string, now time.Time) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	el, ok := l.entries[ip]
	if ok {
		l.lru.MoveToFront(el)
	} else {
		for len(l.entries) >= l.maxEntries {
			oldest := l.lru.Back()
			if oldest == nil {
				break
			}
			delete(l.entries, oldest.Value.(*ipEntry).ip)
			l.lru.Remove(oldest)
		}
		el = l.lru.PushFront(&ipEntry{ip: ip, b: newBucket(l.rate, now)})
		l.entries[ip] = el
	}
	return el.Value.(*ipEntry).b.allow(l.rate, now)
}

// budget is the optional transfer byte budget (contract §6.4). Once used
// bytes reach the budget within the window, transfers answer 429 with
// Retry-After set to the window remainder.
type budget struct {
	mu          sync.Mutex
	bytes       int64
	window      time.Duration
	windowStart time.Time
	used        int64
}

func newBudget(bytes int64, window time.Duration) *budget {
	return &budget{bytes: bytes, window: window, windowStart: time.Now()}
}

// take rolls the window forward, then either reserves n bytes (ok=true) or
// reports how many whole seconds remain until the window resets (ok=false).
func (bd *budget) take(n int64, now time.Time) (retryAfterS int, ok bool) {
	bd.mu.Lock()
	defer bd.mu.Unlock()
	if now.Sub(bd.windowStart) >= bd.window {
		bd.windowStart = now
		bd.used = 0
	}
	if bd.used >= bd.bytes {
		remaining := bd.window - now.Sub(bd.windowStart)
		s := int(remaining / time.Second)
		if remaining%time.Second != 0 || s == 0 {
			s++
		}
		return s, false
	}
	bd.used += n
	return 0, true
}

// add records bytes actually transferred (upload path, where the count is
// only known after the drain).
func (bd *budget) add(n int64) {
	bd.mu.Lock()
	defer bd.mu.Unlock()
	bd.used += n
}
