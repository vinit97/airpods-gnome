# Behavior overview

Controls depend on the AirPods model. AirPods Max uses one battery and wearing
state instead of separate earbuds and a case.

| Action or event | Result |
| --- | --- |
| Connect or disconnect | The indicator appears once the control connection is ready and hides on disconnect. The backend watches for reconnection. |
| Select a listening mode | Updates immediately; a matching AirPods report confirms it. Unsupported modes are hidden. Scroll or right-click the indicator to cycle modes. |
| Change controls quickly | The latest queued choice wins; stale replies cannot undo it. The Adaptive slider follows the pointer and combines intermediate changes. |
| A command fails or times out | The control quietly returns to the reported value. Technical details stay in logs. |
| Conversation Awareness detects speech | Music continues at about 20% of its previous volume, rounded to five-percentage-point steps, with no added start delay. |
| Speech ends or Conversation Awareness is disabled | Restores volume only if the output is the same and its volume was not changed manually. Temporary failures retry up to six times, 1.5 seconds apart; renewed speech cancels a pending restoration. |
| Ear Detection Off | Stops automatic playback control without resuming paused music. |
| Ear Detection I | Removing either AirPod pauses playback; both must be in to resume. |
| Ear Detection II | Removing both AirPods pauses playback; either may be reinserted to resume. |
| Remove both AirPods | A 1.2-second settling period precedes pausing and releasing the audio profile, when Ear Detection is enabled and release is appropriate. |
| Reinsert AirPods | Once the ear condition and playback output are ready, resumes only players previously paused by the backend. |
| Battery updates | Readings update independently. Unknown values show a dash; case readings may stay stale until the case transmits again. |
| Use the AirPods microphone | Preserves the headset profile, including during muted calls. |
| Restart or reconnect | Retains Ear Detection. Conversation Awareness and Adaptive preferences are remembered, but live reports take precedence. Listening mode comes from the AirPods. |
| Stop the backend | Attempts to restore lowered volume, releases Bluetooth discovery, and removes runtime status and the command socket. |

The Adaptive slider is available in Adaptive mode. Ear Detection supports clicks,
Space, and arrow keys. For installation and dependencies, see the
[project README](../README.md).
