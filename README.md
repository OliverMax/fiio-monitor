# fiio-monitor

A native Rust app for listening to and recording audio from a FiiO DM13 on macOS.

Connect the player's **USB REC** port to your Mac, select **USB OUT** on the DM13, and start playback on the player. The app receives the `KT-USB-Audio` input. It does not control the CD transport or switch tracks.

## Requirements

- macOS and a Rust toolchain with Cargo.
- Apple's command-line developer tools for native dependencies.
- A FiiO DM13 connected with a USB data cable.

macOS may ask for microphone permission. The app needs audio-input access to capture the selected USB device; it does not automatically switch to the built-in microphone.

## Run

```sh
git clone https://github.com/OliverMax/fiio-monitor.git
cd fiio-monitor
cargo run --locked --features ui --bin fiio-monitor-ui
```

The app opens a mode-selection screen. Audio starts only after you press Start.

### Listen to music

1. Choose **Listen to music**.
2. Select `KT-USB-Audio` under **Source**.
3. Select headphones or built-in speakers under **Listen on**.
4. Press **Start listening**.

**FiiO Volume** controls listening volume from 0 to 100%. Drag the slider, edit the percentage, or scroll over the control. Hold Shift for finer scroll adjustment. Double-click resets to 50%.

- 0% mutes the output.
- 100% passes the original input level.
- The percentage maps linearly to signal amplitude.
- This changes the app's volume, not the player's hardware volume or the Mac's system volume.

The buffer defaults to 150 ms and can be adjusted while stopped. Stop playback before changing input or output. No audio file is created in listening mode.

### Record from FiiO

1. Choose **Record from FiiO**.
2. Select the USB source and click **Choose WAV file**.
3. Pick a new filename and press **Start recording**.
4. Start playback on the DM13.
5. Press **Stop and save** to finish the WAV file.

Recording uses stereo **48 kHz / 16-bit PCM WAV** at the original input level. The listening-volume slider does not affect recordings. Recording mode does not play audio through the Mac's outputs.

Existing files are never overwritten. After saving, the app displays the path and duration based on the number of written audio frames. The **Back** arrow and normal window closure also stop recording and finalize the file. Force-quitting or a crash may leave an incomplete file.

### Navigation and devices

**Back** stops the current session and returns to mode selection. Device lists refresh automatically every two seconds or with **Refresh devices**. If a selected device disappears, the app stops rather than switching to another device. After reconnecting, select the device and press Start again.

## CLI

The command-line version supports listening without a graphical window:

```sh
cargo run --locked -- --list
cargo run --locked -- --input "KT-USB-Audio" --output "External Headphones"
cargo run --locked -- --output "MacBook Pro Speakers" --gain-db -6 --latency-ms 150
```

Device names must exactly match `--list`. CLI defaults are `KT-USB-Audio`, `MacBook Pro Speakers`, -6 dB, and a 150 ms buffer. The CLI retains its gain option in decibels. Press Enter or Ctrl+C to exit. Use the UI for WAV recording.

## Audio and diagnostics

Both devices must accept stereo audio at 48 kHz. Listening uses an in-memory ring buffer and small adaptive playback-rate corrections with linear interpolation to account for independent device clocks. It is intended for listening, not bit-perfect CD extraction.

The input meter shows the incoming signal in dBFS before listening-volume adjustment. Near-full-scale counts indicate frames close to 0 dBFS; they do not by themselves prove audible clipping. Lowering listening volume cannot repair distortion already present in the source.

Dropped-frame, gap, and stream-error counters help identify interruptions. Recording performs disk writes in a separate worker thread. On an audio or buffer error, it stops and attempts to finalize the captured portion with a warning. WAV recording stops before the standard RIFF 4 GiB size limit. Disk or finalization errors can leave an incomplete file.

## Verification status

The UI and CLI have been built and exercised on macOS with a connected DM13. Live headphone and built-in-speaker streams opened without buffer gaps or stream errors during bounded checks. Slider scrolling and percentage entry were checked in the UI. After Back, CoreAudio confirmed that the app's audio input and output were inactive.

The WAV-recording workflow, recording duration/content, recording finalization on Back or window closure, disk-error handling, device-disconnection handling, and long-duration stability still need end-to-end verification. There is no automated test suite yet.

## Implementation

- `src/lib.rs`: device discovery, live monitoring, volume and diagnostics.
- `src/recorder.rs`: buffered WAV writing and finalization.
- `src/main.rs`: listening CLI.
- `src/bin/ui.rs`: mode selection and native UI.

Uses CPAL, ringbuf, hound, egui/eframe, rfd, and coreaudio-rs. On macOS, output discovery uses CoreAudio directly to include devices that CPAL 0.15.3's format enumeration can omit. Streams are explicitly paused when stopped.

This is an independent project and is not affiliated with or endorsed by FiiO.

## License

Copyright (C) 2026 Serhii Nekrasov.

fiio-monitor is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See [LICENSE](LICENSE) for the full GNU General Public License.

Distributed derivative versions must retain the GPL and provide corresponding source code to recipients as required by the license. Private modifications need not be published. Commercial use is permitted under the same terms. Dependencies retain their respective licenses.
