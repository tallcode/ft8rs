# Using ft8rs

ft8rs receives and decodes FT8. The GUI is a live soundcard monitor; the CLI
supports both live monitoring and WAV files. Build and launch instructions are
in the [README](../README.md) and [development guide](DEVELOPMENT.md).

## GUI workflow

Open **Settings** from the menu or in-window control. Settings is a resizable
window with seven pages:

| Page | Controls and information |
|---|---|
| Station | My Callsign and My Grid |
| Audio | Input device, channel, actual opened format, recent peaks and per-slot capture timing |
| Decode | Profile, Hybrid cross cancellation, SWL and focused deep decoding |
| Frequency | Lower and upper audio-frequency bounds |
| Output | Outgoing UDP decode reports, host and port |
| Compare | Incoming WSJT-X UDP feed and frequency matching tolerance |
| Advanced | JTDX narrow-band filter, duplicate and unresolved-hash display settings |

Choose the receiver input in Audio, select a profile, and press the main-window
play button. Capture is aligned to UTC 15-second slots. The button becomes a
stop control while aligning or monitoring. The message list shows UTC, SNR, DT,
audio frequency and message; new rows appear without waiting for every worker.
Right-click the list to clear it.

The main-window **Callsign**, **Grid** and **RX** fields describe the target and
focused receive frequency. They are separate from your own station settings.
Double-clicking a decoded row fills target context and frequency. In DX, a target
call is required before monitoring can start; changing a running DX target asks
to discard its collected target information. Entering a new target call clears
the previous target grid.

Settings are retained between runs. Text edits commit when focus leaves the
field; selections and toggles apply when changed. Running decode changes take
effect at a slot boundary and can reset AP memory. Device/channel changes reopen
capture and realign it while retaining the decode session. A failed device switch
keeps the previous input and reports the error. Output changes rebuild only the
output sink. Controls stay responsive while background decoding finishes; an
already-running native decoder call cannot be forcibly interrupted.

DX manages SWL/deep passes internally, so its generic SWL, `nagain` and advanced
filter controls are disabled. The GUI uses the default JTDX partition setting;
explicit partition selection is available in the CLI.

## Audio input and levels

The live input path prefers 48 kHz capture and converts samples to the core's
12 kHz mono stream. Channel selection is explicit:

| GUI / CLI channel | Behavior |
|---|---|
| Mono / `mono` | Request a native mono stream; if unavailable, use the first capture channel |
| Left / `left` | Select the first channel |
| Right / `right` | Select the right channel of a multichannel input |
| Mix / `mix` | Average all capture channels |

Mono is the default. Mixing a source present on only one side can reduce its
amplitude by 6 dB; opposite-phase channels can cancel. Older ft8rs versions
averaged the input for Mono, so recheck the selected channel when comparing an
older recording or setup.

**Settings → Audio** distinguishes the OS default device format from the actual
opened format. During monitoring it shows recent decoder-input peak and native
channel peaks, followed by per-slot capture diagnostics. Levels use **dBFS**:
0 dBFS is full scale. They do not use WSJT-X's positive noise-meter units.
48 kHz conversion uses the WSJT-X `fil4` coefficients; other rates use an
anti-aliasing resampler. Neither path changes decoder thresholds.

On Windows FlexRadio systems, select the receive audio endpoint for the intended
slice, for example `DAX Audio RX 1` or `DAX RX 1 (FlexRadio DAX)`. Verify which
channel actually carries audio using the channel peaks. Hardware-specific DAX
behavior still needs a live comparison with the same endpoint, slice, channel
and signal; synthetic channel/PCM tests do not replace that comparison.

For WAV files, multichannel samples are averaged before resampling. The CLI
`--channel` option belongs to live monitoring and does not select a WAV channel;
prepare a mono recording if the file contains different left/right signals.
The final partial WAV slot is zero-padded and decoded.

## Profiles and context

The [README profile table](../README.md#decode-profiles) gives the four modes.
The defaults are WSJT-X, a 200–3000 Hz decode band, and no operator/target context.
Callsign and grid context can assist AP/hash decoding; RX is an audio offset in
Hz, not the radio's RF dial frequency.

Hybrid always has three baseline paths: WSJT-X, JTDX/noAGC and JTDX/AGC.
**Cross cancellation** is enabled by default and can be disabled without removing
those baselines. SWL controls both JTDX paths together. The two JTDX variants are
one algorithm family for evidence accounting.

DX returns target-related messages and manages its own listener/focused recovery.
The target call alone is sufficient to listen; a valid, distinct operator/target
call pair enables additional context recovery. A known grid can enable list
recovery. Derived rows supply weak hints rather than independent confirmation.

GUI provenance markers include `sic` for isolated cancellation recovery, `ctx`
for context recovery and `exp` for the opt-in Hybrid experimental receiver.
The native routines do not expose every AP subtype. Unknown native provenance
has no visible tag and is retained as unknown internally.
These markers describe how a row was obtained, not proof that the message is true.

JTDX uses one logical frequency partition by default. `--jtdx-threads N` changes
partitioning/search state and currently processes those bands serially within a
session; it does not launch N OS threads. Explicit zero enables CPU-dependent
automatic selection. It is separate from FFT thread configuration. SWL-on and
SWL-off are different searches and need not produce nested message sets.

## CLI

```sh
target/release/ft8rs --help
target/release/ft8rs monitor --help
target/release/ft8rs file --help
target/release/ft8rs --version
target/release/ft8rs license
```

Tagged release builds print the release version. Development builds append the
Git revision and, when applicable, a dirty-worktree suffix.

### Live monitoring

```sh
# Omitting --device lists inputs rather than starting capture
target/release/ft8rs monitor

target/release/ft8rs monitor --device 0 --channel mono --slots 2
target/release/ft8rs monitor --device "VB-Cable A" --profile jtdx --swl
target/release/ft8rs monitor --device 0 --profile hybrid --no-hybrid-sic

target/release/ft8rs monitor --device 0 --profile dx \
  --my-call F1MLZ --his-call UA3QNA --rx-frequency 1153
```

Device selection accepts the listed index or full device name. Without `--slots`,
monitoring continues until stopped. CLI output streams rows as they are produced:

```text
133430  16 +0.3  2571 W1FC F5BZB -08
133430  -2 -0.8  1197 CQ F5RXL IN94
------ slot done: 21 decodes ------
```

The completion line describes slot completion, not the arrival time of its first
message. The blocking CLI monitor retains its existing real-time task-start
budgets. The GUI uses the hard process watchdog described below. File mode uses
canonical ordering and can select different additional recovery opportunities.

### WAV decoding

```sh
target/release/ft8rs file crates/ft8rs-core/tests/ft8/210703_133430.wav
target/release/ft8rs file recording.wav --start-time 230208_140300 --profile hybrid
```

A `YYMMDD_HHMMSS` timestamp in the filename sets the first slot's UTC time.
Otherwise provide `--start-time YYMMDD_HHMMSS` or `HHMMSS`. Slots advance by 15
seconds through one persistent session, preserving cross-slot decoder memory.
The file command writes console results; UDP output options are monitor-only.

### Option reference

| Option | Alias | Applies to | Meaning/default |
|---|---|---|---|
| `--device` | `-i` | Monitor | Input index or exact name; omitted lists devices |
| `--channel` | | Monitor | `mono` (default), `left`, `right`, `mix` |
| `--slots` | `-S` | Monitor | Slot limit; omitted keeps listening |
| `--start-time` | `-s` | File | First slot timestamp |
| `--profile` | | Both | `wsjtx` (default), `jtdx`, `hybrid`, `dx` |
| `--low` / `--high` | `-L` / `-H` | Both | Audio decode bounds, default 200 / 3000 Hz |
| `--rx-frequency` | `-f` | Both | Focused receive/QSO audio offset in Hz |
| `--my-call` / `--my-grid` | `-c` / `-G` | Both | Operator context |
| `--his-call` / `--his-grid` | `-x` / `-g` | Both | Target context; target call required for DX |
| `--swl` | | Both | JTDX SWL; both JTDX paths in Hybrid; DX manages its own policy |
| `--jtdx-threads` | | Both | 0–24 logical partitions; default 1, zero opts into automatic selection |
| `--nagain` | | Both | JTDX focused deep retry; DX chooses its own focused passes |
| `--no-hybrid-sic` | | Both | Disable Hybrid cross cancellation, retain three baseline paths |
| `--hybrid-experiment` | | Both | Opt in to the private weak-signal/interference branch; requires `--profile hybrid` |
| `--udp` | `-u` | Monitor | Enable outgoing decode reports; default off |
| `--udp-host` / `--udp-port` | `-o` / `-p` | Monitor | Destination, default `127.0.0.1:2238` |

## UDP output and comparison

Outgoing reports are WSJT-X-compatible Decode packets (schema 2, message type 2)
with client ID `ft8rs` and mode `FT8`. Enable them in GUI **Output** or CLI:

```sh
target/release/ft8rs monitor --device 0 --udp \
  --udp-host 127.0.0.1 --udp-port 2238
```

GUI **Compare** is a separate incoming unicast listener, enabled by default at
`127.0.0.1:2237`, with a configurable frequency tolerance (default ±7 Hz).
Configure WSJT-X to send its UDP reports to that address. **Listen to WSJT-X
(UDP)** remembers your choice, including a previously saved disabled setting.
The display merges matching local/external messages
and colors local-only, WSJT-X-only and shared rows; its legend shows the colors.
This feed does not seed the local decoder's hash/AP state or change its results.
New live FT8 Decode packets are accepted, including low-confidence results;
replays, off-air WAV playback and other packet types are ignored. A free
listening port is required.
The incoming Decode mode accepts the native WSJT-X/JTDX symbol `~` as well as
`FT8`. Local and external display/matching use the message body with decoder
annotations such as `?` and `a1` removed. Matching requires the same UTC slot,
compatible message tokens and a frequency difference no greater than the current
**Match ± Hz** setting. SNR, DT and the trailing provenance column do not
participate. Changes to the tolerance apply to subsequently arriving messages;
existing rows are not matched again.

Display comparison is not an independent truth label. For reproducible decoder
acceptance, use identical audio, context and database with the
[per-message test comparison](../crates/ft8rs-core/tests/decoder/README.md).

## Troubleshooting

| Symptom | Check |
|---|---|
| GUI exits before showing a window, while CLI works | Windows defaults to DX12, avoiding automatic Vulkan enumeration. An affected Windows 11 / i5-9600T machine could launch 0.0.24 after setting `WGPU_BACKEND=dx12`. Read `%LOCALAPPDATA%\ft8rs\logs\ft8rs-gui.log` on newer builds. Returned startup graphics errors retry with OpenGL before the app starts; set `FT8RS_RENDERER=glow` to bypass wgpu, including when a driver crashes before it can return an error. The previous session is saved as `ft8rs-gui.previous.log`. |
| No GUI diagnostic log is created | If the executable cannot reach `main` (e.g. a missing imported DLL or a native loader failure), inspect Windows Event Viewer → Windows Logs → Application for the failed executable, module and exception code. |
| No messages and input peak stays silent | Correct receive endpoint, actual opened format, per-channel peaks, microphone/input permission |
| Audio is present but unexpectedly weak | Mono/Left/Right selection, single-sided source mixed down, opposite-phase channels, receiver levels |
| GUI cannot start DX | Fill the target Callsign; your own callsign is optional |
| Different JTDX/Hybrid results across runs or machines | CallDB hash, SWL/AGC, logical partitions, context, FFT backend and build profile |
| Device change fails | Reported capture error; the previous working input remains active |
| WSJT-X comparison is empty | Incoming host/port, port conflicts and new FT8 UDP reports; outgoing port 2238 is a separate setting |
| Audio discontinuity reports increase | CPAL reports capture buffer timing gaps; monitoring continues and Audio shows a cumulative count. This is separate from device-busy, access-denied or device-loss errors, which still stop capture. Windows WASAPI capture uses shared mode. |
| Messages arrive after the display target | Measure first/last callback separately from worker completion and queueing; SWL/deep searches can cost more |

Technical limitations, the known JTDX long-recording miss and the partial
geographic filter are documented in [validation](VALIDATION.md) and the
[JTDX notes](../crates/ft8rs-core/JTDX.md).

The GUI exposes **Experimental decoder** at the bottom of Decode settings,
after Deep decode, for Hybrid.
It is disabled by default and saved across restarts. Enabling it adds the
independent stable weak-signal/interference receiver; supplementary lines end in
`exp`. Native callbacks still arrive first. The branch uses the remaining soft
budget up to roughly ten seconds from slot processing start in file/CLI mode.
GUI monitoring additionally enforces the hard process deadline below. See the [receiver design and evidence](../crates/ft8rs-core/src/decode/experiment/README.md).

An `exp` suffix identifies an additional experimental result; it is not a
per-slot activity indicator. Rows already decoded by a native path retain their
native provenance. Compare mode preserves the local suffix even when the
external WSJT-X UDP row arrived first. The GUI log records whether the experiment
was enabled, input sample count, FEC/accepted counts and budget exhaustion per
completed Hybrid slot, so a disabled branch, an exhausted budget and a search
with no accepted additions can be distinguished.

Windows uses only the DX12 wgpu backend by default. An explicit `WGPU_BACKEND`
still overrides it; other platforms keep their existing backend defaults.
To force the OpenGL renderer for one launch in Windows PowerShell:

```powershell
$env:FT8RS_RENDERER = "glow"
.\ft8rs-gui.exe
```

Use `Remove-Item Env:FT8RS_RENDERER` to restore the default renderer for subsequent
launches from that shell. Normal Windows builds use the OS graphics stack and do
not require FFTW. Startup logs distinguish graphics initialization, application
creation and audio device enumeration; the input stream opens only when
monitoring starts. If the preferred log directory is unwritable, the GUI tries
`logs` beside the executable and then `ft8rs/logs` in the system temporary directory.


### GUI real-time deadline and comparison ordering

The GUI runs live decoding in a persistent child process using private pipes.
Every received callback is forwarded immediately. Each slot has a cumulative
10-second work budget across its early/subtract/final stages, including pipe
transport and process startup, while idle waiting for more audio is excluded.
Queued stages also expire after ten seconds instead of delaying current audio.
The parent checks the deadline at a 5 ms polling granularity. At expiry it kills
and reaps the entire child, keeps the messages already displayed, drops remaining
stages for that slot and uses a fresh native session on the next slot. Stop also
terminates active work immediately. Budget expiry and stale queued work are
handled silently in the GUI and recorded only in the diagnostic log; actual
worker failures still appear as errors. Normal slots keep their native sessions
and normal configuration migration; only a timeout/crash discards learned state.
No cancellation code or thresholds are added to the native decoder algorithms.
The standalone file/CLI decode path remains available for unbounded offline work.

The comparison table consolidates repeated observations from the same UDP source
and repeats received after a row has become `Both`. Matching includes UTC slot,
message, frequency and DT (within 0.3 s), so distinct same-message signals at
different DT are retained. Local measurements/provenance win when a local and
external row merge. A late result is inserted in its original slot section rather
than adding another old separator below the current slot. This grouping does not
claim that an external UDP result itself arrived on time.

WSJT-X's `Network/MessageClient.cpp` suppresses only consecutively identical
serialized packets; changed SNR/DT/annotation fields are different packets, and
multicast can send on multiple interfaces. `widgets/mainwindow.cpp::replayDecodes`
uses `New=false`, which the ft8rs listener excludes. Repeated `New=true` observations
still require display deduplication.
