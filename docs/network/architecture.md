# Network Architecture

## Overview

Scarlet provides an in-kernel network stack with layered protocol composition, OS-agnostic socket abstractions, and VirtIO-net as the primary backend.

## Module Structure

```
kernel/src/network/
├── mod.rs              – NetworkManager, NetworkInterface
├── socket.rs           – SocketObject trait, SocketDomain/Type/Protocol
├── protocol_stack.rs   – NetworkLayer trait, ProtocolStackManager
├── tcp.rs              – Full TCP (handshake, flow control, retransmission)
├── udp.rs              – UDP datagram layer
├── ip.rs               – Common IPv4/IPv6 address values
├── ipv6.rs             – IPv6 base headers and static routing
├── ipv4.rs             – IPv4 layer
├── icmp.rs             – ICMP layer
├── arp.rs              – ARP cache
├── ethernet.rs         – Ethernet II frame layer
├── ethernet_interface.rs – Interface management
├── local.rs            – Local stream/sequenced-packet sockets
├── local_datagram.rs   – Bounded local datagram records and transfers
├── link.rs             – Link identity, incarnation and statistics
├── traffic.rs          – Packet accounting and reservations
├── traffic_policy.rs   – Named packet meters and policy
├── config.rs           – Interface configuration (IP, gateway)
└── syscall.rs          – Socket syscall dispatch

kernel/src/device/network/  – NetworkDevice trait
kernel/src/drivers/network/
└── virtio_net.rs        – VirtIO-net driver
```

## Layered Architecture

```text
Application (via ABI syscall)
    │
    ▼
SocketObject (OS-agnostic: bind/connect/listen/read/write)
    │
    ▼
ProtocolStack / NetworkLayer
    │
    ├── TCP Layer (port 6)    ── full 3-way handshake, flow control, retransmission
    ├── UDP Layer (port 17)   ── datagram send/receive
    ├── ICMP Layer (port 1)   ── ping support
    │
    ▼
IPv4 Layer (routing, fragmentation)
    │
    ▼
Ethernet Layer (MAC, EtherType demux)
    │
    ▼
NetworkDevice (VirtIO-net, future: physical NIC)
```

Each layer implements the `NetworkLayer` trait and communicates through `LayerContext` — a protocol-agnostic key-value routing context. Layers are registered with `ProtocolStackManager` and composed at runtime.

## Socket Abstraction

Scarlet defines OS-agnostic socket types:

| Type | Domain | Description |
|------|--------|-------------|
| `Stream` | `Local` | Unix-domain stream sockets |
| `Stream` | `Inet4` | TCP sockets |
| `Datagram` | `Inet4` | UDP sockets |
| `Datagram` | `Local` | Unix-domain datagram sockets |
| `Raw` | `Inet4` | Raw IP sockets |

ABI modules (Linux, xv6) translate their specific syscall conventions to `SocketObject` calls. The kernel does not expose Linux-specific socket options directly.

## NetworkManager

`NetworkManager` is the global singleton that owns:

- Socket lifecycle (creation, tracking by ID)
- Interface registry (named interfaces with IP/MAC)
- Default gateway and routing
- ARP cache
- Socket factories registered by ABI modules

## VirtIO-net Driver

The VirtIO-net driver (`kernel/src/drivers/network/virtio_net.rs`) implements `NetworkDevice` and `EthernetDevice`:

- Two virtqueues: receive (index 0) and transmit (index 1)
- MAC address from device features or configurable
- MTU management
- Link status detection
- Integrates with `EthernetLayer` for frame transmission

## Interface Configuration

```rust
// Programmatic configuration
manager.set_ip(interface_name, Ipv4Address::new(10, 0, 2, 15));
manager.set_subnet_mask(interface_name, Ipv4Address::new(255, 255, 255, 0));
manager.set_gateway(Ipv4Address::new(10, 0, 2, 2));
```

Configuration and DHCP lifecycle are managed through the native interfaces
described in [network management](management.md).

## Protocol Support Summary

| Protocol | Status |
|----------|--------|
| Ethernet II | Implemented |
| ARP | Implemented |
| IPv4 | Implemented |
| ICMP | Implemented |
| TCP | Implemented (full handshake, flow control, retransmission) |
| UDP | Implemented |
| Local/Unix sockets | Implemented |
| IPv6 | Address, base-header and static-routing foundation; full sockets and neighbor discovery are not implemented. |

## IPv6 boundaries

[ip.rs](../../kernel/src/network/ip.rs) retains native 128-bit addresses;
IPv4-mapped addresses do not implicitly turn an IPv6 socket into an IPv4 socket.
[ipv6.rs](../../kernel/src/network/ipv6.rs) validates base headers, selects static
routes and dispatches supported payloads. Non-local transmission requires an
explicit destination Ethernet address until neighbor discovery is integrated.
Extension-header processing and fragment reassembly remain unsupported.
Linux socket creation rejects unsupported IPv6 rather than fabricating an IPv4
socket. The IPv4 diagram above describes the established socket path.

## Accounting and local IPC

[Link statistics](../../kernel/src/network/link.rs) validate interface identity
and incarnation, not just a reusable interface name. Native requests and
responses are defined in [scarlet-abi](../../user/lib/scarlet-abi/src/network.rs).
Linux read-only sysfs network views translate those native counters.

[Traffic budgets](../../kernel/src/network/traffic.rs) retain reservations for
queued packets and return credit on cancelled or failed sends. Named
[packet meters](../../kernel/src/network/traffic_policy.rs) provide bounded
accounting and notifications, including the Linux quota adapter. They do not
provide Linux BPF execution: unsupported BPF operations are rejected.

[Local datagrams](../../kernel/src/network/local_datagram.rs) carry bounded
records and retained sender/descriptor state. Linux handles SCM_RIGHTS and
credential wire translation in its ABI layer. Sequenced-packet accept preserves
the socket type; blocking socket operations use deadlines so a wake does not
restart the full timeout. These are common networking mechanisms, not
Android-only services.
