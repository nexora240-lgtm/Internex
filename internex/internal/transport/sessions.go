package transport

import (
	"net/http"
	"strings"
	"sync"
	"time"
)

// ---------------------------------------------------------------------------
// Session store — per-origin virtualized state
// ---------------------------------------------------------------------------

// SessionStore holds virtualized browser state keyed by upstream origin
// (e.g. "https://example.com").  It is safe for concurrent use.
type SessionStore struct {
	mu       sync.RWMutex
	origins  map[string]*OriginSession
}

// OriginSession holds cookies and key-value storage for a single origin.
type OriginSession struct {
	mu            sync.RWMutex
	Cookies       []*http.Cookie
	LocalStorage  map[string]string
	SessionStorage map[string]string
}

// Global default session store.
var DefaultSessions = NewSessionStore()

// NewSessionStore creates an empty session store.
func NewSessionStore() *SessionStore {
	return &SessionStore{
		origins: make(map[string]*OriginSession),
	}
}

// getOrCreate returns the OriginSession for the given origin, creating one
// if necessary.
func (s *SessionStore) getOrCreate(origin string) *OriginSession {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if ok {
		return sess
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	// Double-check after acquiring write lock.
	if sess, ok = s.origins[origin]; ok {
		return sess
	}
	sess = &OriginSession{
		Cookies:        nil,
		LocalStorage:   make(map[string]string),
		SessionStorage: make(map[string]string),
	}
	s.origins[origin] = sess
	return sess
}

// ---------------------------------------------------------------------------
// Cookie jar operations
// ---------------------------------------------------------------------------

// SetCookiesFromResponse parses Set-Cookie headers from an upstream
// response and stores them in the per-origin jar.
func (s *SessionStore) SetCookiesFromResponse(origin string, resp *http.Response) {
	cookies := resp.Cookies()
	if len(cookies) == 0 {
		return
	}
	sess := s.getOrCreate(origin)
	sess.mu.Lock()
	defer sess.mu.Unlock()

	for _, c := range cookies {
		replaced := false
		for i, existing := range sess.Cookies {
			if existing.Name == c.Name && strings.EqualFold(existing.Path, c.Path) {
				sess.Cookies[i] = c
				replaced = true
				break
			}
		}
		if !replaced {
			sess.Cookies = append(sess.Cookies, c)
		}
	}
}

// CookieHeader builds a Cookie header value to send to the upstream
// origin, filtering out expired cookies.
func (s *SessionStore) CookieHeader(origin string) string {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return ""
	}

	sess.mu.RLock()
	defer sess.mu.RUnlock()

	now := time.Now()
	var parts []string
	for _, c := range sess.Cookies {
		// Skip expired cookies.
		if !c.Expires.IsZero() && c.Expires.Before(now) {
			continue
		}
		parts = append(parts, c.Name+"="+c.Value)
	}
	return strings.Join(parts, "; ")
}

// GetCookies returns a copy of the stored cookies for an origin.
func (s *SessionStore) GetCookies(origin string) []*http.Cookie {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return nil
	}

	sess.mu.RLock()
	defer sess.mu.RUnlock()

	out := make([]*http.Cookie, len(sess.Cookies))
	copy(out, sess.Cookies)
	return out
}

// DeleteCookie removes a named cookie from the origin's jar.
func (s *SessionStore) DeleteCookie(origin, name string) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return
	}

	sess.mu.Lock()
	defer sess.mu.Unlock()

	for i, c := range sess.Cookies {
		if c.Name == name {
			sess.Cookies = append(sess.Cookies[:i], sess.Cookies[i+1:]...)
			return
		}
	}
}

// ---------------------------------------------------------------------------
// Storage operations (localStorage / sessionStorage)
// ---------------------------------------------------------------------------

// SetLocalStorage sets a key-value pair in the origin's localStorage.
func (s *SessionStore) SetLocalStorage(origin, key, value string) {
	sess := s.getOrCreate(origin)
	sess.mu.Lock()
	defer sess.mu.Unlock()
	sess.LocalStorage[key] = value
}

// GetLocalStorage retrieves a value from the origin's localStorage.
func (s *SessionStore) GetLocalStorage(origin, key string) (string, bool) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return "", false
	}
	sess.mu.RLock()
	defer sess.mu.RUnlock()
	v, found := sess.LocalStorage[key]
	return v, found
}

// DeleteLocalStorage removes a key from the origin's localStorage.
func (s *SessionStore) DeleteLocalStorage(origin, key string) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return
	}
	sess.mu.Lock()
	defer sess.mu.Unlock()
	delete(sess.LocalStorage, key)
}

// ClearLocalStorage wipes all localStorage for an origin.
func (s *SessionStore) ClearLocalStorage(origin string) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return
	}
	sess.mu.Lock()
	defer sess.mu.Unlock()
	sess.LocalStorage = make(map[string]string)
}

// SetSessionStorage sets a key-value pair in the origin's sessionStorage.
func (s *SessionStore) SetSessionStorage(origin, key, value string) {
	sess := s.getOrCreate(origin)
	sess.mu.Lock()
	defer sess.mu.Unlock()
	sess.SessionStorage[key] = value
}

// GetSessionStorage retrieves a value from the origin's sessionStorage.
func (s *SessionStore) GetSessionStorage(origin, key string) (string, bool) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return "", false
	}
	sess.mu.RLock()
	defer sess.mu.RUnlock()
	v, found := sess.SessionStorage[key]
	return v, found
}

// DeleteSessionStorage removes a key from the origin's sessionStorage.
func (s *SessionStore) DeleteSessionStorage(origin, key string) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return
	}
	sess.mu.Lock()
	defer sess.mu.Unlock()
	delete(sess.SessionStorage, key)
}

// ClearSessionStorage wipes all sessionStorage for an origin.
func (s *SessionStore) ClearSessionStorage(origin string) {
	s.mu.RLock()
	sess, ok := s.origins[origin]
	s.mu.RUnlock()
	if !ok {
		return
	}
	sess.mu.Lock()
	defer sess.mu.Unlock()
	sess.SessionStorage = make(map[string]string)
}

// ClearAll wipes the entire session store.
func (s *SessionStore) ClearAll() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.origins = make(map[string]*OriginSession)
}