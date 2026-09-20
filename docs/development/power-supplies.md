# Power-supply telemetry

`/dev/power_supply` is a read-only control device. Hardware drivers register
`device::power_supply::PowerSupply` providers. The kernel gives each provider a
boot-stable ID and reads it without holding the global registry lock. A failed
read preserves the provider identity and returns no stale measurements.

The ABI lives in `scarlet-abi::power_supply`; `scarlet_os::power_supply::PowerSupplies`
provides the owning user-space wrapper. Batteries and external sources are
enumerated separately. Unsupported measurements are `None`, including on
machines with no suitable hardware driver. Input availability, battery charge
state and battery percentage are separate observations.

## Version 1

`SCTL_POWER_SUPPLY_COUNT` returns the device count (currently at most 16).
`SCTL_POWER_SUPPLY_SNAPSHOT` takes a pointer to a 96-byte input/output buffer.
The input starts with three little-endian u32 fields: version 1, size 96, ID.
The driver rejects unknown versions, sizes and IDs. Both copies use the
kernel's user-copy helpers; the wire encoder never copies Rust padding.

| Byte offset | Field |
| --- | --- |
| 0, 4, 8 | Version, size, ID (u32) |
| 12 | Supply kind (u32) |
| 16 | Flags: bit 0 means hardware read failed |
| 20 | Validity bits for the eight fields at offsets 32..60 |
| 24 | Monotonic observation time in ns (u64) |
| 32, 36 | Present, online (u32 booleans) |
| 40 | Charge state (u32) |
| 44 | Battery capacity in tenths of a percent (0..1000) |
| 48 | Voltage in microvolts (u32) |
| 52 | Battery current in microamps (i32, positive into battery) |
| 56 | Temperature in millidegrees Celsius (i32) |
| 60 | Configured input-current limit in microamps (u32) |
| 64 | NUL-terminated UTF-8 name in 32 bytes |

`online` means the external source is supplying power, not merely that a cable
exists. The configured input-current limit is not a measured input current or
a USB-PD contract. Charging is reported independently, so a connected adapter
can coexist with a battery discharging under load.

## Desktop

Scarlet Shell polls in its existing background status worker once every five
iterations (nominally five seconds). Its status bar puts one Tabler battery
icon immediately after volume. It never adds percentage text to that bar:

- `battery`, `battery-1` through `battery-4`: battery capacity bands.
- `battery-charging`: charging.
- `battery-charging-2`: external power without net battery charging.
- `battery-exclamation`: known battery with unavailable capacity.

Control Center shows the percentage and power state. The first present battery
is selected by ID; multiple percentages are not averaged without energy-capacity
data. `power-info` lists every provider and the additional electrical quantities.

## Verification

The ABI tests cover the fixed wire layout, signed fields, missing versus zero
data, malformed headers and failed reads. The standalone host test package
`user/tests/power-supply` tests the shell's actual presentation policy, including
charging versus plugged-in and partial external-source failures.
