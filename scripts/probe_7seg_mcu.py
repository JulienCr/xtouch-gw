"""
Probe the X-Touch 7-segment timecode display in MCU mode.

Mackie Control Protocol uses CC 0x40-0x4B (64-75) — one CC per digit,
12 digits total (left-to-right).

CC value encoding (per the spec):
  bit 7: always 0
  bit 6: decimal-point on/off
  bits 5-0: character code
    0..31  -> ASCII 64..95 (i.e. '@' 'A'..'Z' '[' '\' ']' '^' '_')
    32..63 -> ASCII 32..63 (i.e. ' ' '!' .. '0'..'9' .. '?')

Encoding helper: take ASCII byte; if 64..95, subtract 64; else keep.
"""

import mido
import time
import sys

def encode_char(ch: str, dot: bool = False) -> int:
    """Map an ASCII char to its MCU 7-seg CC value."""
    code = ord(ch.upper())
    if 64 <= code <= 95:
        v = code - 64
    elif 32 <= code <= 63:
        v = code
    else:
        v = 32  # space for anything out of range
    if dot:
        v |= 0x40
    return v

def find_port(substr: str) -> str:
    outs = mido.get_output_names()
    for n in outs:
        if substr.lower() in n.lower():
            return n
    print(f"ERROR: port {substr!r} not found"); sys.exit(1)

CC_FIRST = 0x40  # left-most digit
CC_COUNT = 12

def write_text(port, text: str):
    text = text.ljust(CC_COUNT)[:CC_COUNT]
    for i, ch in enumerate(text):
        v = encode_char(ch)
        port.send(mido.Message("control_change", channel=0, control=CC_FIRST + i, value=v))

def main():
    port_name = find_port("X-Touch")
    # Prefer the first matching port (not MIDIOUT2)
    outs = mido.get_output_names()
    candidates = [n for n in outs if "x-touch" in n.lower()]
    primary = next((n for n in candidates if "midiout2" not in n.lower()), candidates[0])
    print(f"Using port: {primary!r}\n")

    with mido.open_output(primary) as port:

        print("TEST 1: 'HELLO XTOUC' (12 chars left-to-right) — 4s")
        write_text(port, "HELLO XTOUC")
        time.sleep(4)

        print("TEST 2: 'OBS LIVE    ' — 4s")
        write_text(port, "OBS LIVE")
        time.sleep(4)

        print("TEST 3: '012345678901' — 4s")
        write_text(port, "012345678901")
        time.sleep(4)

        print("TEST 4: only leftmost digit = 'A' (CC 64 value 1) — 3s")
        # First clear, then write single digit
        for i in range(CC_COUNT):
            port.send(mido.Message("control_change", channel=0, control=CC_FIRST + i, value=32))
        port.send(mido.Message("control_change", channel=0, control=CC_FIRST, value=encode_char('A')))
        time.sleep(3)

        print("TEST 5: rightmost digit = 'Z' (CC 75 value 26) — 3s")
        port.send(mido.Message("control_change", channel=0, control=CC_FIRST + 11, value=encode_char('Z')))
        time.sleep(3)

        print("CLEANUP: all spaces — 1s")
        write_text(port, "")
        time.sleep(1)

    print("\nDone. Tell me which tests displayed text on the timecode.")

if __name__ == "__main__":
    main()
