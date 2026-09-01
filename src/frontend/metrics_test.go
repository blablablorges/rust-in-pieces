package main

import (
	"strings"
	"testing"
)

func TestSplitFullMethod(t *testing.T) {
	for _, tc := range []struct{ in, service, method string }{
		{"/hipstershop.CartService/GetCart", "hipstershop.CartService", "GetCart"},
		{"/hipstershop.ShippingService/GetQuote", "hipstershop.ShippingService", "GetQuote"},
		// Unexpected shapes are kept whole rather than dropped, so a gRPC
		// naming change shows up in the output instead of vanishing.
		{"weird", "weird", ""},
		{"", "", ""},
	} {
		s, m := splitFullMethod(tc.in)
		if s != tc.service || m != tc.method {
			t.Errorf("splitFullMethod(%q) = (%q, %q), want (%q, %q)", tc.in, s, m, tc.service, tc.method)
		}
	}
}

func TestWriteMetricsExpositionFormat(t *testing.T) {
	rpcMu.Lock()
	rpcCounts = map[rpcKey]uint64{
		{"hipstershop.CartService", "GetCart", "OK"}:          3,
		{"hipstershop.CartService", "GetCart", "Unavailable"}: 1,
		{"hipstershop.ShippingService", "GetQuote", "OK"}:     2,
	}
	rpcMu.Unlock()
	defer func() {
		rpcMu.Lock()
		rpcCounts = make(map[rpcKey]uint64)
		rpcMu.Unlock()
	}()

	var b strings.Builder
	writeMetrics(&b)
	got := b.String()

	for _, want := range []string{
		"# TYPE frontend_grpc_client_requests_total counter",
		`frontend_grpc_client_requests_total{grpc_service="hipstershop.CartService",grpc_method="GetCart",grpc_code="OK"} 3`,
		`frontend_grpc_client_requests_total{grpc_service="hipstershop.CartService",grpc_method="GetCart",grpc_code="Unavailable"} 1`,
		`frontend_grpc_client_requests_total{grpc_service="hipstershop.ShippingService",grpc_method="GetQuote",grpc_code="OK"} 2`,
	} {
		if !strings.Contains(got, want) {
			t.Errorf("missing line:\n  %s\ngot:\n%s", want, got)
		}
	}

	// Both outcomes of the same call must be present: a treatment that starts
	// failing shows as a shift between codes, not as a missing series.
	if strings.Count(got, `grpc_method="GetCart"`) != 2 {
		t.Errorf("expected OK and Unavailable for GetCart, got:\n%s", got)
	}
}

func TestEscapeLabel(t *testing.T) {
	if got := escapeLabel(`a"b\c`); got != `a\"b\\c` {
		t.Errorf("escapeLabel = %q", got)
	}
}
