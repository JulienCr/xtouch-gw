# Mackie Control Universal (MCU) — 7-segment / timecode display

The 7-segment **timecode** display on the right of the X-Touch is driven
by standard Mackie Control Protocol (MCU) — **not** SysEx. Sources:

- TouchMCU community docs:
  <https://github.com/NicoG60/TouchMCU/blob/main/doc/mackie_control_protocol.md>
- Logic Control / Emagic manual (original MCU spec — not freely
  redistributable, hence not bundled).
- The Xctl-mode equivalent (Behringer native, not what we use) is in
  `xctl-protocol-v1.0.pdf` next to this file, pages 8–9.

## Wire format

Twelve **CC messages** on **channel 1** (status byte `0xB0`):

| CC      | Hex    | Display digit position |
|---------|--------|------------------------|
| 75      | `0x4B` | left-most              |
| 74      | `0x4A` | ...                    |
| 73      | `0x49` | ...                    |
| …       | …      | …                      |
| 65      | `0x41` | ...                    |
| 64      | `0x40` | right-most             |

**Order is reversed** vs. reading order: write the right-most character
to `0x40` and walk left toward `0x4B`. The X-Touch hardware does the
ASCII-to-segments decoding itself; we never send raw segment bit
patterns.

## CC-value encoding (one digit)

```
bit 7  bit 6  bits 5..0
  0      D      C C C C C C
```

- **bit 7**: always `0` (MIDI requires CC values in `0..=127`).
- **bit 6** (`D`): decimal-point on/off for this digit.
- **bits 5..0** (`C`): the 6-bit character code.

Character code mapping (only the upper-case ASCII range fits in 6 bits):

| Character range   | ASCII     | CC value       |
|-------------------|-----------|----------------|
| `@`, `A`..`Z`, `[\]^_` | 64..95 | 0..31 (`ascii - 64`) |
| ` `, `!`..`?`, `0`..`9` | 32..63 | 32..63 (`ascii` direct) |
| anything else     | —         | fall back to `32` (space) |

So to display `OBS LIVE` left-aligned:

| Pos | Char | ASCII | CC value | CC#   |
|-----|------|-------|----------|-------|
| 0   | `O`  | 79    | 15       | `0x4B` |
| 1   | `B`  | 66    | 2        | `0x4A` |
| 2   | `S`  | 83    | 19       | `0x49` |
| 3   | ` `  | 32    | 32       | `0x48` |
| 4   | `L`  | 76    | 12       | `0x47` |
| 5   | `I`  | 73    | 9        | `0x46` |
| 6   | `V`  | 86    | 22       | `0x45` |
| 7   | `E`  | 69    | 5        | `0x44` |
| 8–11| ` `  | 32    | 32       | `0x43..0x40` |

## What this **does not** cover

- The MCU spec also exposes a SysEx command (`0x10`/`0x11`) for the time
  display in some clones. The X-Touch in MCU mode does **not** respond
  to those (confirmed empirically). CC is the only working path.
- The Behringer `00 20 32 dd 37 …` SysEx documented in
  `xctl-protocol-v1.0.pdf` is for **pure Xctl mode** — silently ignored
  when the X-Touch is set to MC/MCU.
- The assignment / mode indicators on the left of the timecode are CC
  64..65 (right-most two of the 12-digit field above) in MCU; native
  Xctl gives them separate CCs (96/97).

## Reference implementation

See `XTouchDriver::set_seven_segment_text` and
`XTouchDriver::encode_seven_seg_cc` in `src/xtouch.rs`.
