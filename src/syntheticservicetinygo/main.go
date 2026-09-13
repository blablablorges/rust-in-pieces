// Synthetic service in Go for TinyGo's wasip2 target: a wasi:http 0.2 proxy component serving
// POST /workload with the same stressors, levels and sizes as the Rust guests' core.rs, and
// GET /healthz for the readiness probe. TinyGo's wasip2 has no net.Listen, so there is no gRPC
// server; outbound gRPC goes through the host's wasi:http client, redis through wasi:sockets.
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strconv"
	"time"

	"github.com/ydnar/wasi-http-go/wasihttp"
)

type level int

const (
	none level = iota
	mid
	high
)

func levelFromEnv(key string) level {
	switch os.Getenv(key) {
	case "1":
		return mid
	case "2":
		return high
	}
	return none
}

func env(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func runCPU(l level) string {
	limit := map[level]int{mid: 10_000, high: 200_000}[l]
	if limit == 0 {
		return "skipped"
	}
	sieve := make([]bool, limit+1)
	for i := range sieve {
		sieve[i] = true
	}
	sieve[0], sieve[1] = false, false
	for i := 2; i*i <= limit; i++ {
		if sieve[i] {
			for j := i * i; j <= limit; j += i {
				sieve[j] = false
			}
		}
	}
	count := 0
	for _, p := range sieve {
		if p {
			count++
		}
	}
	return fmt.Sprintf("found %d primes ≤ %d", count, limit)
}

func runMemory(l level) string {
	size := map[level]int{mid: 1 << 20, high: 8 << 20}[l]
	if size == 0 {
		return "skipped"
	}
	buf := make([]byte, size)
	for i := range buf {
		buf[i] = 0xAB
	}
	var checksum uint64
	for i := 0; i < size; i += 4096 {
		checksum += uint64(buf[i])
	}
	buf[0] = byte(checksum & 0xFF)
	return fmt.Sprintf("allocated and touched %d MiB, checksum %d", size>>20, checksum)
}

func runIO(l level) string {
	size := map[level]int{mid: 1 << 20, high: 16 << 20}[l]
	if size == 0 {
		return "skipped"
	}
	dir := env("WORKLOAD_TMP_DIR", "/tmp")
	path := fmt.Sprintf("%s/loadserwasm-probe-%s-%d", dir, env("REQUEST_ID", "0"), time.Now().UnixNano())
	data := bytes.Repeat([]byte{0xAB}, size)
	if err := os.WriteFile(path, data, 0o644); err != nil {
		os.Remove(path)
		return "write error: " + err.Error()
	}
	data = nil
	read, err := os.ReadFile(path)
	os.Remove(path)
	if err != nil {
		return "read error: " + err.Error()
	}
	return fmt.Sprintf("wrote+read %d MiB", len(read)>>20)
}

var client = &http.Client{Transport: &wasihttp.Transport{}}

// One unary ListProducts over the host's wasi:http client: gRPC framing by hand, the product
// count read by counting field-1 entries of ListProductsResponse (no protobuf runtime).
func listProductsOnce(addr string) (int, error) {
	req, err := http.NewRequest("POST", "http://"+addr+"/hipstershop.ProductCatalogService/ListProducts", bytes.NewReader(make([]byte, 5)))
	if err != nil {
		return 0, err
	}
	req.Header.Set("content-type", "application/grpc")
	req.Header.Set("te", "trailers")
	resp, err := client.Do(req)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return 0, err
	}
	status := resp.Trailer.Get("grpc-status")
	if status == "" {
		status = resp.Header.Get("grpc-status")
	}
	if status != "0" {
		return 0, fmt.Errorf("grpc-status %q, http %d", status, resp.StatusCode)
	}
	if len(body) < 5 {
		return 0, fmt.Errorf("short gRPC frame: %d bytes", len(body))
	}
	return countField1(body[5:])
}

func countField1(msg []byte) (int, error) {
	n, i := 0, 0
	for i < len(msg) {
		tag, w := uvarint(msg[i:])
		if w == 0 {
			return n, fmt.Errorf("bad tag")
		}
		i += w
		field, wire := tag>>3, tag&7
		switch wire {
		case 0:
			_, w = uvarint(msg[i:])
			i += w
		case 1:
			i += 8
		case 2:
			l, w := uvarint(msg[i:])
			i += w + int(l)
			if field == 1 {
				n++
			}
		case 5:
			i += 4
		default:
			return n, fmt.Errorf("unsupported wire type %d", wire)
		}
	}
	return n, nil
}

func uvarint(b []byte) (uint64, int) {
	var x uint64
	for i, c := range b {
		x |= uint64(c&0x7f) << (7 * uint(i))
		if c < 0x80 {
			return x, i + 1
		}
	}
	return 0, 0
}

func runNetwork(l level) string {
	calls := map[level]int{mid: 3, high: 10}[l]
	if calls == 0 {
		return "skipped"
	}
	addr := env("PRODUCT_CATALOG_SERVICE_ADDR", "productcatalogservice:3550")
	total, errors := 0, 0
	for i := 0; i < calls; i++ {
		n, err := listProductsOnce(addr)
		if err != nil {
			errors++
			continue
		}
		total += n
	}
	return fmt.Sprintf("%d calls, %d products, %d errors", calls, total, errors)
}

func runDB(l level) string {
	rounds := map[level]int{mid: 5, high: 20}[l]
	if rounds == 0 {
		return "skipped"
	}
	addr := env("REDIS_ADDR", "redis-cart:6379")
	errors := 0
	for i := 0; i < rounds; i++ {
		if err := redisRoundtrip(addr, fmt.Sprintf("synthetic:probe:%d", i), []byte("loadserwasm")); err != nil {
			errors++
		}
	}
	return fmt.Sprintf("%d roundtrips, %d errors", rounds, errors)
}

func init() {
	http.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-length", "2")
		w.Write([]byte("ok"))
	})
	http.HandleFunc("/workload", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		start := time.Now()
		results := map[string]string{
			"cpu":     runCPU(levelFromEnv("CPU_LEVEL")),
			"memory":  runMemory(levelFromEnv("MEMORY_LEVEL")),
			"io":      runIO(levelFromEnv("IO_LEVEL")),
			"network": runNetwork(levelFromEnv("NETWORK_LEVEL")),
			"db":      runDB(levelFromEnv("DB_LEVEL")),
		}
		body, _ := json.Marshal(map[string]any{"stressor_results": results, "total_ms": time.Since(start).Milliseconds()})
		w.Header().Set("content-type", "application/json")
		w.Header().Set("content-length", strconv.Itoa(len(body)))
		w.Write(body)
	})
}

func main() {}
