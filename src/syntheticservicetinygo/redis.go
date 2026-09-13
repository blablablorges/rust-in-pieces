package main

// RESP SET/GET over wasi:sockets 0.2 through the generated bindings under internal/wasi: one
// connection per roundtrip, as the Rust guests' db mode does. TinyGo's wasip2 standard
// library has no net.Dial, so this is the only way to reach redis from the guest.

import (
	"fmt"
	"net"
	"strconv"

	"go.bytecodealliance.org/cm"

	instancenetwork "syntheticservicetinygo/internal/wasi/sockets/instance-network"
	ipnamelookup "syntheticservicetinygo/internal/wasi/sockets/ip-name-lookup"
	"syntheticservicetinygo/internal/wasi/sockets/network"
	"syntheticservicetinygo/internal/wasi/sockets/tcp"
	tcpcreatesocket "syntheticservicetinygo/internal/wasi/sockets/tcp-create-socket"
)

func resolve(host string) (network.IPAddress, error) {
	if ip := net.ParseIP(host); ip != nil {
		if v4 := ip.To4(); v4 != nil {
			return network.IPAddressIPv4(network.IPv4Address{v4[0], v4[1], v4[2], v4[3]}), nil
		}
		var a network.IPv6Address
		for i := 0; i < 8; i++ {
			a[i] = uint16(ip[2*i])<<8 | uint16(ip[2*i+1])
		}
		return network.IPAddressIPv6(a), nil
	}
	r := ipnamelookup.ResolveAddresses(instancenetwork.InstanceNetwork(), host)
	if r.IsErr() {
		return network.IPAddress{}, fmt.Errorf("resolve %s: %v", host, *r.Err())
	}
	stream := *r.OK()
	defer stream.ResourceDrop()
	for {
		next := stream.ResolveNextAddress()
		if next.IsErr() {
			if *next.Err() == network.ErrorCodeWouldBlock {
				p := stream.Subscribe()
				p.Block()
				p.ResourceDrop()
				continue
			}
			return network.IPAddress{}, fmt.Errorf("resolve %s: %v", host, *next.Err())
		}
		opt := *next.OK()
		if opt.None() {
			return network.IPAddress{}, fmt.Errorf("no address for %s", host)
		}
		return *opt.Some(), nil
	}
}

type conn struct {
	sock tcp.TCPSocket
	in   tcp.InputStream
	out  tcp.OutputStream
}

func dial(addr string) (*conn, error) {
	host, portStr, err := net.SplitHostPort(addr)
	if err != nil {
		return nil, err
	}
	port, err := strconv.Atoi(portStr)
	if err != nil {
		return nil, fmt.Errorf("bad port in %s", addr)
	}
	ip, err := resolve(host)
	if err != nil {
		return nil, err
	}
	var family network.IPAddressFamily
	var remote network.IPSocketAddress
	if v4 := ip.IPv4(); v4 != nil {
		family = network.IPAddressFamilyIPv4
		remote = network.IPSocketAddressIPv4(network.IPv4SocketAddress{Port: uint16(port), Address: *v4})
	} else {
		family = network.IPAddressFamilyIPv6
		remote = network.IPSocketAddressIPv6(network.IPv6SocketAddress{Port: uint16(port), Address: *ip.IPv6()})
	}
	cr := tcpcreatesocket.CreateTCPSocket(family)
	if cr.IsErr() {
		return nil, fmt.Errorf("socket: %v", *cr.Err())
	}
	sock := *cr.OK()
	if sc := sock.StartConnect(instancenetwork.InstanceNetwork(), remote); sc.IsErr() {
		sock.ResourceDrop()
		return nil, fmt.Errorf("connect %s: %v", addr, *sc.Err())
	}
	for {
		fc := sock.FinishConnect()
		if fc.IsErr() {
			if *fc.Err() == network.ErrorCodeWouldBlock {
				p := sock.Subscribe()
				p.Block()
				p.ResourceDrop()
				continue
			}
			sock.ResourceDrop()
			return nil, fmt.Errorf("connect %s: %v", addr, *fc.Err())
		}
		pair := *fc.OK()
		return &conn{sock: sock, in: pair.F0, out: pair.F1}, nil
	}
}

func (c *conn) close() {
	c.in.ResourceDrop()
	c.out.ResourceDrop()
	c.sock.ResourceDrop()
}

// Write one command, read until a whole reply is in the buffer.
func (c *conn) exchange(payload []byte) ([]byte, error) {
	if w := c.out.BlockingWriteAndFlush(cm.ToList(payload)); w.IsErr() {
		return nil, fmt.Errorf("redis: write failed")
	}
	var buf []byte
	for {
		if n, err := respLen(buf); err != nil {
			return nil, err
		} else if n > 0 {
			return buf[:n], nil
		}
		r := c.in.BlockingRead(4096)
		if r.IsErr() {
			return nil, fmt.Errorf("redis: peer closed mid-reply")
		}
		chunk := r.OK().Slice()
		if len(chunk) == 0 {
			return nil, fmt.Errorf("redis: peer closed mid-reply")
		}
		buf = append(buf, chunk...)
	}
}

func respEncode(args ...[]byte) []byte {
	buf := []byte(fmt.Sprintf("*%d\r\n", len(args)))
	for _, a := range args {
		buf = append(buf, []byte(fmt.Sprintf("$%d\r\n", len(a)))...)
		buf = append(buf, a...)
		buf = append(buf, '\r', '\n')
	}
	return buf
}

func crlf(b []byte) int {
	for i := 0; i+1 < len(b); i++ {
		if b[i] == '\r' && b[i+1] == '\n' {
			return i
		}
	}
	return -1
}

// Length of the first complete reply in buf, 0 if none yet.
func respLen(buf []byte) (int, error) {
	if len(buf) == 0 {
		return 0, nil
	}
	switch buf[0] {
	case '$':
		pos := crlf(buf)
		if pos < 0 {
			return 0, nil
		}
		n, err := strconv.Atoi(string(buf[1:pos]))
		if err != nil {
			return 0, fmt.Errorf("redis: bad bulk length")
		}
		if n < 0 {
			return pos + 2, nil
		}
		if total := pos + 2 + n + 2; len(buf) >= total {
			return total, nil
		}
		return 0, nil
	case '+', '-', ':':
		if pos := crlf(buf); pos >= 0 {
			return pos + 2, nil
		}
		return 0, nil
	}
	return 0, fmt.Errorf("redis: unexpected reply type %#x", buf[0])
}

func redisRoundtrip(addr, key string, value []byte) error {
	c, err := dial(addr)
	if err != nil {
		return err
	}
	defer c.close()
	for _, cmd := range [][]byte{respEncode([]byte("SET"), []byte(key), value), respEncode([]byte("GET"), []byte(key))} {
		reply, err := c.exchange(cmd)
		if err != nil {
			return err
		}
		if reply[0] == '-' {
			return fmt.Errorf("redis: %s", reply[1:crlf(reply)])
		}
	}
	return nil
}
