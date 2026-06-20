// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! The MQTT command topic -> campaign planner routing (C8 #36, U5).
//!
//! Home Assistant publishes a JSON command to the configured command topic to
//! start, pause, or adjust a campaign; this module parses it into a typed
//! [`CampaignCommand`] and [`apply`](CampaignCommand::apply)s it to the U2
//! [`CampaignPlanner`].
//!
//! The JSON shape (HA automations publish this) is an internally-tagged action:
//!
//! ```json
//! { "action": "start",  "campaignId": "<uuid>" }
//! { "action": "pause",  "campaignId": "<uuid>" }
//! { "action": "adjust", "campaignId": "<uuid>", "threshold": 0.75 }
//! ```
//!
//! Parsing fails closed: a malformed or unknown command is a
//! [`CoreError::Mqtt`] the poll task logs and drops, never crashing the bridge.

use serde::{Deserialize, Serialize};

use crate::campaigns::{Campaign, CampaignPlanner};
use crate::error::{CoreError, Result};

/// A campaign control command received over the MQTT command topic (C8 #36).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[serde(rename_all_fields = "camelCase")]
pub enum CampaignCommand {
    /// Start (or resume) a campaign by id.
    Start {
        /// The target campaign id.
        campaign_id: String,
    },
    /// Pause a campaign by id.
    Pause {
        /// The target campaign id.
        campaign_id: String,
    },
    /// Adjust a campaign's goal threshold.
    Adjust {
        /// The target campaign id.
        campaign_id: String,
        /// The new goal threshold (must be finite when applied).
        threshold: f64,
    },
}

impl CampaignCommand {
    /// Parse a command from a raw command-topic payload (JSON bytes). A
    /// malformed/unknown payload is a [`CoreError::Mqtt`] (fail closed).
    pub fn parse(payload: &[u8]) -> Result<Self> {
        serde_json::from_slice(payload)
            .map_err(|e| CoreError::Mqtt(format!("malformed campaign command: {e}")))
    }

    /// The campaign id this command targets.
    pub fn campaign_id(&self) -> &str {
        match self {
            CampaignCommand::Start { campaign_id }
            | CampaignCommand::Pause { campaign_id }
            | CampaignCommand::Adjust { campaign_id, .. } => campaign_id,
        }
    }

    /// Route this command into the U2 [`CampaignPlanner`], returning the updated
    /// campaign. `now` is the wall-clock the resulting state change is stamped
    /// with.
    pub async fn apply(&self, planner: &CampaignPlanner, now: i64) -> Result<Campaign> {
        match self {
            CampaignCommand::Start { campaign_id } => planner.start(campaign_id, now).await,
            CampaignCommand::Pause { campaign_id } => planner.pause(campaign_id, now).await,
            CampaignCommand::Adjust {
                campaign_id,
                threshold,
            } => planner.adjust_threshold(campaign_id, *threshold, now).await,
        }
    }
}

/// Parse and apply a raw command-topic payload to the planner in one step (the
/// poll task's inbound handler). Returns the updated campaign on success.
pub async fn route(planner: &CampaignPlanner, payload: &[u8], now: i64) -> Result<Campaign> {
    let command = CampaignCommand::parse(payload)?;
    command.apply(planner, now).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_start_pause_adjust() -> Result<()> {
        let start = CampaignCommand::parse(br#"{"action":"start","campaignId":"abc"}"#)?;
        assert_eq!(
            start,
            CampaignCommand::Start {
                campaign_id: "abc".to_string()
            }
        );
        assert_eq!(start.campaign_id(), "abc");

        let pause = CampaignCommand::parse(br#"{"action":"pause","campaignId":"abc"}"#)?;
        assert_eq!(
            pause,
            CampaignCommand::Pause {
                campaign_id: "abc".to_string()
            }
        );

        let adjust =
            CampaignCommand::parse(br#"{"action":"adjust","campaignId":"abc","threshold":0.5}"#)?;
        assert_eq!(
            adjust,
            CampaignCommand::Adjust {
                campaign_id: "abc".to_string(),
                threshold: 0.5
            }
        );
        Ok(())
    }

    #[test]
    fn command_round_trips() -> Result<()> {
        let cmd = CampaignCommand::Adjust {
            campaign_id: "id".to_string(),
            threshold: 1.25,
        };
        let json = serde_json::to_string(&cmd)?;
        let back = CampaignCommand::parse(json.as_bytes())?;
        assert_eq!(back, cmd);
        Ok(())
    }

    #[test]
    fn malformed_command_fails_closed() {
        assert!(matches!(
            CampaignCommand::parse(b"not json"),
            Err(CoreError::Mqtt(_))
        ));
        assert!(matches!(
            CampaignCommand::parse(br#"{"action":"explode","campaignId":"x"}"#),
            Err(CoreError::Mqtt(_))
        ));
    }
}
