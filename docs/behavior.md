# Behavior overview

The Rust backend receives AirPods events through Bluetooth, updates the GNOME
extension, and controls playback and volume through the desktop's audio services.

| Action or event | Result |
| --- | --- |
| AirPods connect | The backend reads their settings and batteries. The top-bar indicator appears after the control connection is ready. |
| AirPods disconnect | The indicator disappears. Preferences remain saved. The backend watches for reconnection. |
| Click a listening mode | The selection updates immediately. The backend waits for a matching AirPods report before confirming the change. Unsupported modes are hidden. |
| Scroll or right-click the indicator | Cycles through supported listening modes. |
| Change a selection quickly | The latest queued choice replaces earlier queued choices for that control. An older failed request cannot undo the latest selection. |
| Move the Adaptive slider | The percentage follows the pointer. Commands are briefly combined while moving; unchanged rounded percentages are not resent. The slider is available in Adaptive mode. |
| A control change fails | The control quietly returns to the reported value. Technical details stay in logs. |
| Turn Conversation Awareness on | Enables the AirPods' conversation detection. |
| AirPods report a conversation starting | Music continues at roughly 20% of its previous volume, rounded to five-percentage-point steps. The backend adds no deliberate start delay. |
| AirPods report the conversation ending | The backend restores the previous volume if the output still matches and its volume has not been changed manually. |
| Volume restoration temporarily fails | The backend retries up to six times, 1.5 seconds apart. Renewed speech cancels the pending restoration. |
| Turn Conversation Awareness off | Disables detection and restores volume lowered by the backend. |
| Set Ear Detection to Off | Automatic playback control stops. This does not force paused music to resume. |
| Set Ear Detection to I | Removing either AirPod pauses playback; both must be in to resume. |
| Set Ear Detection to II | Both AirPods must be out to pause; either one being in permits resuming. |
| Remove both AirPods | With Ear Detection enabled, a 1.2-second settling period filters brief removal events before pausing and releasing the audio profile when appropriate. |
| Reinsert AirPods | Once the selected ear condition is met and playback output is ready, the backend resumes only players that it previously paused. |
| A battery update arrives | Left, case, and right readings update independently. An unavailable reading shows a dash. Case readings can remain stale until the case transmits again. |
| Start a call | The backend preserves a headset profile while an application is using the AirPods microphone. |
| Restart or reconnect | Ear Detection is retained. Conversation Awareness and Adaptive preferences are remembered, but live AirPods reports take precedence. Listening mode comes from the AirPods. |
| Stop the backend | It attempts to restore lowered volume, releases its Bluetooth discovery session, and removes its runtime status and command socket. |

AirPods Max uses one headset battery and wearing state instead of separate earbuds
and a case. Controls vary with the model's capabilities.

## Dependencies

The repository includes the extension, Rust backend, and icon source. No separate
LibrePods or Omapods installation is needed. Runtime integration uses GNOME,
BlueZ, PipeWire/WirePlumber, D-Bus, and systemd. Building requires Rust/Cargo and the
dependencies pinned in `daemon/Cargo.lock`; Cargo downloads those libraries.
