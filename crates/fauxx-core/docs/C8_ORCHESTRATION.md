# C8 Orchestration Core (idle scheduling + campaigns + Home Assistant)

This note documents the three orchestration-core features that turn the always-on
headless companion into a goal-aware, observable, hub-controllable system. All
three live in `fauxx-core` behind the clean async `Core` API; the GUI and CLI are
thin clients (the tray agent + first-run wizard U3 and the homelab serve mode U4
are separate later batches and add no types here).

Everything is 100% local (no telemetry), persists behind SQLCipher where it
persists at all, FAILS CLOSED, and holds NO GUI/CLI types.

## U1: idle / lock-aware scheduling (#32, `crate::idle`)

The companion runs decoy browsing 24/7, so it must yield to the human: run
HEAVIER when the box is idle, and PAUSE (or throttle) the instant the user is
back or the session is locked.

- `IdleState` is the three-way model: `Active`, `Idle(Duration)`, `Locked`.
- `IdleSource` (`#[async_trait]`, `Send + Sync`) is the injectable detection
  seam. `StubIdleSource` drives the tests; `ConservativeIdleSource` is the
  dep-free default that reports `Active` whenever real detection is unavailable.
- `RatePlanner` maps a sampled state + a base `IntensityLevel` to a
  `RateDecision` (`Paused` or `Run(level)`): an idle threshold crossed SCALES UP
  one ladder step per whole threshold-multiple of idle (clamped at `Extreme`);
  `Active`/`Locked` apply the configurable `ActiveBehavior` (`Pause` by default,
  or `Throttle(floor)`). Both the threshold and the active behavior are
  configurable via `IdleScalingConfig`.

The planner owns gating only; the C1 household timeline scheduler still samples
the Poisson stream at the resulting rate.

### Per-OS detection is a documented gap (follow-up)

Real per-OS idle/lock detection is a deliberate follow-up, not built here. The
trait is the stable seam the backends slot into without changing the rate
planner:

- Linux: `org.freedesktop.login1` `IdleHint`/`IdleSinceHint` over D-Bus, the
  Wayland `ext-idle-notify` protocol, or X11 `XScreenSaverQueryInfo`.
- Windows: `GetLastInputInfo` for idle time plus `WTSRegisterSessionNotification`
  (`WM_WTSSESSION_CHANGE`) for lock/unlock.
- macOS: `IOHIDSystem` `HIDIdleTime` plus `CGSessionCopyCurrentDictionary`
  (`kCGSessionOnConsoleKey`) for lock state.

Each needs an OS-specific optional dependency wired in a `target.'cfg(...)'`
block behind a feature, exactly as the keystore backends are. Until that lands
the conservative default keeps the contract sound: the planner never ramps
without a real idle signal (it errs toward not over-running while the user might
be active). No idle-detection crate is added now.

## U2: goal-driven campaigns (#33, `crate::campaigns`)

A `Campaign` aims at a measurable target ("drop the TECH segment's drift below
X") and stops once it holds, instead of running raw intensity forever.

- `Goal` = (`TargetMetric`, `Comparator`, threshold). The only metric today is
  `SegmentDrift` (the C4 A1 KL-divergence drift for the target segment); the enum
  is `#[non_exhaustive]` for future signals. A non-finite threshold is refused.
- `Campaign` carries the goal, the target segment/category, the persona, a
  lifecycle (`Planned -> Running -> Achieved`, `Paused` on user request from any
  active state), and the closed-loop `CampaignProgress`.
- The closed loop (`Campaign::tick`): compute the signed GAP to the threshold,
  map it to a `CampaignDirective` (intensity + target-segment bias) via
  `directive_for_gap`, BACKING OFF as the metric nears the threshold (within
  `BACKOFF_GAP`), and flip to `Achieved` once the goal has held continuously for
  the configurable dwell (`DEFAULT_DWELL_MS`).
- `MetricSource` is the closed-loop signal seam: `StubMetricSource` for tests,
  `MeasurementMetricSource` for production (it reads the latest per-segment drift
  contribution from the C4 A1 heatmap for the persona's Google Topics platform).
- `CampaignPlanner` persists campaigns + their progress in the `campaigns` table
  (schema `v14 -> v15`, forward-only via the `PRAGMA user_version` pattern) so a
  campaign survives restart with its dwell clock intact.

The `Core` facade exposes `save/get/list/delete/start/pause/adjust/tick` over the
planner, plus `tick_running_campaigns`.

## U5: Home Assistant / MQTT hooks (#36, `crate::mqtt`)

An always-on homelab deployment is observable and controllable from the homelab's
hub. This mirrors the cinder `RumqttcBridge` pattern.

- `MqttBridge` (`#[async_trait]`, `Send + Sync`) is the seam. `MockMqtt` records
  (and logs) publishes and ALWAYS compiles (no rumqttc). The real `RumqttcBridge`
  lives behind the off-by-default `mqtt` cargo feature, so the DEFAULT headless
  build links no MQTT client. `cargo tree -p fauxx-core -e features` shows no
  rumqttc without `--features mqtt`.
- The real bridge (`mqtt::real`, `#[cfg(feature = "mqtt")]`): `connect` builds the
  SINGLE shared `AsyncClient` + `EventLoop`, spawns ONE poll task that
  re-subscribes the command topic on every `ConnAck` and routes inbound via a
  bounded mpsc `try_send` (drop + warn, never block), and warns + sleeps on a poll
  error (rumqttc auto-reconnects). A DOWN broker is NON-fatal: the core degrades.
  The publish handle is a CLONED `AsyncClient` (a request-channel sender), NEVER an
  `Arc<Mutex>`; a dropped/failed publish WARNS and never crashes.
- Status and efficacy (the A1 drift summary) publish as Home Assistant
  MQTT-DISCOVERY sensors (`discovery::DiscoveryConfig`, `SensorPayload`,
  `StatusPayload`, `EfficacySensor`) under a configurable base topic + discovery
  prefix (`MqttConfig`).
- The command topic carries `start` / `pause` / `adjust` JSON
  (`mqtt::command::CampaignCommand`), parsed fail-closed and routed into the U2
  `CampaignPlanner` (`mqtt::command::route`, surfaced on `Core` as
  `route_campaign_command` / `apply_campaign_command`).

All MQTT config (host, port, base topic, discovery prefix, device id, optional
credentials) lives in the plain `MqttConfig` type, no GUI/CLI types.

## Tests

`tests/orchestration_core.rs` is hermetic (temp `EncryptedFile` store, no broker,
no live measurement, no OS idle detection):

- U1: the rate planner gated across `Active` / `Idle(>threshold)` / `Locked` via
  the stub, plus the configurable active behavior.
- U2: the closed loop (gap, intensity, back-off near threshold, Achieved/Paused)
  with a stubbed metric source, a persistence round-trip, and survival across a
  store reopen (schema migrated forward).
- U5: the `MockMqtt` bridge asserts the HA-discovery sensor + state payloads, and
  a command-topic message routes start/pause/adjust into the campaign planner.

Plus per-module unit tests in `idle`, `campaigns`, and `mqtt`.
