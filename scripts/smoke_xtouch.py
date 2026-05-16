"""
Sanity check: send 3 known-good MIDI messages to the X-Touch.
If you don't see any of these on the hardware, we have the wrong port.
"""

import mido
import time
import sys

def find_xtouch_output() -> str:
    outs = mido.get_output_names()
    print("Available MIDI outputs:")
    for name in outs:
        print(f"  - {name!r}")
    candidates = [n for n in outs if "x-touch" in n.lower() or "xtouch" in n.lower()]
    if not candidates:
        print("ERROR: no X-Touch port found"); sys.exit(1)
    # If multiple, ask user
    if len(candidates) > 1:
        print(f"\nMultiple X-Touch ports detected: {candidates}")
        print("Trying the first one. If it fails, edit the script to try another.")
    return candidates[0]

def main():
    port_name = find_xtouch_output()
    print(f"\nOpening: {port_name!r}\n")

    with mido.open_output(port_name) as port:

        # --- TEST 1: Light up the PLAY button LED (note 94, channel 1) ---
        # MCU standard: PLAY = note 0x5E (94). NoteOn vel=127 = LED on.
        print("TEST 1: PLAY button LED ON (note 94, vel 127). Press <Enter> when verified.")
        port.send(mido.Message("note_on", channel=0, note=94, velocity=127))
        input()

        print("        PLAY button LED OFF (vel 0). Press <Enter>.")
        port.send(mido.Message("note_on", channel=0, note=94, velocity=0))
        input()

        # --- TEST 2: Write 'HELLO!' to LCD strip 1 upper line (Mackie SysEx) ---
        # F0 00 00 66 14 12 [pos=0] [7 ASCII bytes] F7
        # mido takes the data WITHOUT F0/F7 framing.
        print("TEST 2: LCD strip 1 upper line = 'HELLO!'. Press <Enter> when verified.")
        text = b"HELLO! "[:7]  # 7-char wide LCD
        port.send(mido.Message(
            "sysex",
            data=[0x00, 0x00, 0x66, 0x14, 0x12, 0x00] + list(text),
        ))
        input()

        # --- TEST 3: Set all 8 LCD strip colors to RED ---
        # F0 00 00 66 14 72 [8 colors, 1=red] F7
        print("TEST 3: All 8 LCD strips colored RED. Press <Enter> when verified.")
        port.send(mido.Message(
            "sysex",
            data=[0x00, 0x00, 0x66, 0x14, 0x72] + [0x01] * 8,
        ))
        input()

        # --- Cleanup: back to BLACK + clear LCD line ---
        print("Cleanup: black LCDs + clear strip 1.")
        port.send(mido.Message(
            "sysex",
            data=[0x00, 0x00, 0x66, 0x14, 0x72] + [0x00] * 8,
        ))
        port.send(mido.Message(
            "sysex",
            data=[0x00, 0x00, 0x66, 0x14, 0x12, 0x00] + [0x20] * 7,
        ))

    print("\nDone. Tell me which of 1/2/3 worked.")

if __name__ == "__main__":
    main()
