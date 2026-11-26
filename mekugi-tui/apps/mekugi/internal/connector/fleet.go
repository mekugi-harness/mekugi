package connector

import (
	"context"
	"crypto/subtle"
	json "encoding/json/v2"
	"errors"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
)

const (
	FleetPath        = "/api/fleet"
	fleetProbeBudget = 3 * time.Second
)

// FleetNode mirrors mekugi-mcp DEVBOXES_JSON / DEVBOXES_FILE node entries.
type FleetNode struct {
	Name        string            `json:"name"`
	Endpoint    string            `json:"endpoint,omitempty"`
	Target      string            `json:"target,omitempty"`
	Labels      map[string]string `json:"labels,omitempty"`
	Description string            `json:"description,omitempty"`
	Default     bool              `json:"default,omitempty"`
	SandboxKit  string            `json:"sandboxKit,omitempty"`
}

// FleetNodeStatus is returned by GET /api/fleet with live reachability and latency.
type FleetNodeStatus struct {
	Name        string            `json:"name"`
	Endpoint    string            `json:"endpoint"`
	Labels      map[string]string `json:"labels,omitempty"`
	Description string            `json:"description,omitempty"`
	Default     bool              `json:"default,omitempty"`
	SandboxKit  string            `json:"sandboxKit,omitempty"`
	Reachable   bool              `json:"reachable"`
	LatencyMs   int64             `json:"latencyMs,omitempty"`
	Error       string            `json:"error,omitempty"`
}

// ParseFleetJSON parses either a JSON array of FleetNode or {"boxes": [...]}.
func ParseFleetJSON(data []byte) ([]FleetNode, error) {
	trimmed := strings.TrimSpace(string(data))
	if trimmed == "" {
		return nil, nil
	}
	var nodes []FleetNode
	if strings.HasPrefix(trimmed, "[") {
		if err := json.Unmarshal([]byte(trimmed), &nodes); err != nil {
			return nil, err
		}
	} else {
		var wrapper struct {
			Boxes []FleetNode `json:"boxes"`
			Nodes []FleetNode `json:"nodes"`
		}
		if err := json.Unmarshal([]byte(trimmed), &wrapper); err != nil {
			return nil, err
		}
		if len(wrapper.Boxes) > 0 {
			nodes = wrapper.Boxes
		} else {
			nodes = wrapper.Nodes
		}
	}
	out := make([]FleetNode, 0, len(nodes))
	for _, node := range nodes {
		node.Name = strings.TrimSpace(node.Name)
		node.Endpoint = strings.TrimSpace(node.Endpoint)
		if node.Endpoint == "" {
			node.Endpoint = strings.TrimSpace(node.Target)
		}
		if node.Name == "" || node.Endpoint == "" {
			continue
		}
		out = append(out, node)
	}
	return out, nil
}

// LoadFleetFromEnv reads DEVBOXES_JSON or DEVBOXES_FILE when configured.
func LoadFleetFromEnv() ([]FleetNode, error) {
	if raw := strings.TrimSpace(os.Getenv("DEVBOXES_JSON")); raw != "" {
		return ParseFleetJSON([]byte(raw))
	}
	if file := strings.TrimSpace(os.Getenv("DEVBOXES_FILE")); file != "" {
		data, err := os.ReadFile(file)
		if err != nil {
			return nil, err
		}
		return ParseFleetJSON(data)
	}
	return nil, nil
}

// ProbeFleet checks live reachability and latency for each HTTP/HTTPS devbox endpoint.
func ProbeFleet(ctx context.Context, client *http.Client, nodes []FleetNode) []FleetNodeStatus {
	if client == nil {
		client = &http.Client{Timeout: fleetProbeBudget}
	}
	results := make([]FleetNodeStatus, len(nodes))
	var wg sync.WaitGroup
	for i, node := range nodes {
		wg.Add(1)
		go func(idx int, n FleetNode) {
			defer wg.Done()
			status := FleetNodeStatus{
				Name:        n.Name,
				Endpoint:    n.Endpoint,
				Labels:      n.Labels,
				Description: n.Description,
				Default:     n.Default,
				SandboxKit:  n.SandboxKit,
			}
			probeURL, err := fleetProbeURL(n.Endpoint)
			if err != nil {
				status.Error = err.Error()
				results[idx] = status
				return
			}
			probeCtx, cancel := context.WithTimeout(ctx, fleetProbeBudget)
			defer cancel()
			req, err := http.NewRequestWithContext(probeCtx, http.MethodGet, probeURL, nil)
			if err != nil {
				status.Error = err.Error()
				results[idx] = status
				return
			}
			req.Header.Set(sessionapi.ProtocolHeader, sessionapi.ProtocolVersion)
			start := time.Now()
			resp, err := client.Do(req)
			if err != nil {
				status.Error = "unreachable"
				results[idx] = status
				return
			}
			_ = resp.Body.Close()
			status.LatencyMs = max(time.Since(start).Milliseconds(), 1)
			if resp.StatusCode == http.StatusOK {
				status.Reachable = true
			} else {
				status.Error = resp.Status
			}
			results[idx] = status
		}(i, node)
	}
	wg.Wait()
	return results
}

func fleetProbeURL(endpoint string) (string, error) {
	u, err := url.Parse(endpoint)
	if err != nil || u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") {
		return "", errors.New("endpoint must be an http or https URL")
	}
	return strings.TrimRight(u.Scheme+"://"+u.Host+u.Path, "/") + "/api/version", nil
}

func (c *Connector) fleet(w http.ResponseWriter, r *http.Request) {
	if origin := r.Header.Get("Origin"); origin != "" {
		w.Header().Set("Access-Control-Allow-Origin", origin)
	}
	if r.Method == http.MethodOptions {
		w.Header().Set("Access-Control-Allow-Methods", "GET")
		w.Header().Set("Access-Control-Allow-Headers", "Authorization, "+sessionapi.ProtocolHeader)
		w.WriteHeader(http.StatusNoContent)
		return
	}
	if r.Method != http.MethodGet {
		http.NotFound(w, r)
		return
	}
	c.mu.Lock()
	credentials := r.Header.Values("Authorization")
	valid := len(credentials) == 1 && strings.HasPrefix(credentials[0], "Bearer ") && subtle.ConstantTimeCompare([]byte(strings.TrimPrefix(credentials[0], "Bearer ")), []byte(c.token)) == 1
	if !valid {
		c.mu.Unlock()
		http.Error(w, "A valid connector token is required.", http.StatusUnauthorized)
		return
	}
	if c.closed || c.connections >= connectionLimit {
		c.mu.Unlock()
		http.Error(w, "The connector cannot accept another request now.", http.StatusServiceUnavailable)
		return
	}
	gen := c.generation
	c.mu.Unlock()
	nodes, err := LoadFleetFromEnv()
	if err != nil {
		http.Error(w, "Invalid devbox fleet configuration.", http.StatusInternalServerError)
		return
	}
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	stop := context.AfterFunc(gen, cancel)
	defer stop()
	statuses := ProbeFleet(ctx, nil, nodes)
	w.Header().Set("Content-Type", "application/json")
	payload, err := json.Marshal(struct {
		Nodes []FleetNodeStatus `json:"nodes"`
	}{Nodes: statuses})
	if err != nil {
		http.Error(w, "Could not encode fleet status.", http.StatusInternalServerError)
		return
	}
	_, _ = w.Write(payload)
}
