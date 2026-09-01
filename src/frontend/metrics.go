package main

// Per-backend gRPC call counters for the frontend, in Prometheus text format.
//
// Why here and only here: the frontend calls every backend and is runc in every
// treatment, so counting at this one place is symmetric across the ladder and
// adds nothing to the wasm pods whose cost is being measured. Counting inside
// the wasm services is not an option — containerd-shim-wasmtime builds a fresh
// Store and instance per request (http_proxy.rs, handle_request), so an
// in-process counter in a guest resets on every call.
//
// Hand-rolled rather than using the OTel Prometheus exporter: that exporter and
// prometheus/client_golang are not among this module's dependencies, and the
// exposition format needed here is three lines of fmt. The OTel metric SDK is
// present transitively but wiring a MeterProvider only to re-export it would
// pull in the same modules.
//
// OFF unless ENABLE_METRICS=1. Turning it on changes the frontend from the
// configuration the published numbers were produced with, so enable it for a
// whole campaign or not at all — never for some treatments and not others.

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
	"sync"

	"google.golang.org/grpc"
	"google.golang.org/grpc/status"
)

type rpcKey struct {
	service string
	method  string
	code    string
}

var (
	rpcMu     sync.Mutex
	rpcCounts = make(map[rpcKey]uint64)
)

// splitFullMethod turns "/hipstershop.CartService/GetCart" into its two parts.
// Anything unexpected is kept whole under service so a format change shows up
// in the output instead of being silently dropped.
func splitFullMethod(full string) (service, method string) {
	trimmed := strings.TrimPrefix(full, "/")
	if i := strings.LastIndex(trimmed, "/"); i >= 0 {
		return trimmed[:i], trimmed[i+1:]
	}
	return trimmed, ""
}

// metricsUnaryInterceptor counts every outbound unary RPC by gRPC status code.
// status.Code(nil) is OK, so successes and failures share one counter family
// and a treatment that starts failing shows as a shift between codes rather
// than as a gap someone has to notice.
func metricsUnaryInterceptor(
	ctx context.Context,
	fullMethod string,
	req, reply any,
	cc *grpc.ClientConn,
	invoker grpc.UnaryInvoker,
	opts ...grpc.CallOption,
) error {
	err := invoker(ctx, fullMethod, req, reply, cc, opts...)
	service, method := splitFullMethod(fullMethod)
	rpcMu.Lock()
	rpcCounts[rpcKey{service, method, status.Code(err).String()}]++
	rpcMu.Unlock()
	return err
}

// escapeLabel applies the three escapes the Prometheus text format requires.
var labelEscaper = strings.NewReplacer(`\`, `\\`, `"`, `\"`, "\n", `\n`)

func escapeLabel(s string) string { return labelEscaper.Replace(s) }

const metricName = "frontend_grpc_client_requests_total"

// writeMetrics emits the exposition format. Sorted so a diff between two
// scrapes is readable by a human, which is the main way this gets used.
func writeMetrics(w io.Writer) {
	rpcMu.Lock()
	lines := make([]string, 0, len(rpcCounts))
	for k, v := range rpcCounts {
		lines = append(lines, fmt.Sprintf(
			"%s{grpc_service=%q,grpc_method=%q,grpc_code=%q} %d",
			metricName, escapeLabel(k.service), escapeLabel(k.method), escapeLabel(k.code), v))
	}
	rpcMu.Unlock()
	sort.Strings(lines)

	fmt.Fprintf(w, "# HELP %s Outbound unary gRPC calls made by the frontend, by backend and status code.\n", metricName)
	fmt.Fprintf(w, "# TYPE %s counter\n", metricName)
	for _, l := range lines {
		fmt.Fprintln(w, l)
	}
}

func metricsHandler(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
	writeMetrics(w)
}
