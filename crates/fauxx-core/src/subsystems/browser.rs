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

//! Browser-automation subsystem seam.
//!
//! This trait pins the "run one synthetic browsing session for a persona" seam
//! the scheduler calls into. The real implementation now lives in
//! [`crate::browser`] (C2 #11 R1 / #13 R3), which launches an isolated decoy
//! Chromium profile over CDP. [`StubBrowserDriver`] remains as the no-op default
//! for store-less/smoke contexts; [`DecoyBrowserDriver`] is the real driver that
//! delegates to [`crate::browser::DecoyBrowser`].

use async_trait::async_trait;

use crate::browser::{BrowserLaunchConfig, DecoyBrowser};
use crate::error::{CoreError, Result};
use crate::persona::SyntheticPersona;

/// Runs a single synthetic browsing session for a persona.
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    /// Run one browsing session for the persona with the given id.
    async fn run_session(&self, _persona_id: &str) -> Result<()> {
        Err(CoreError::Unimplemented("browser::run_session"))
    }
}

/// No-op browser driver used in store-less/smoke contexts. Kept so callers that
/// must not touch a real browser (tests, headless boot before a profile exists)
/// have a safe default.
#[derive(Debug, Clone, Copy, Default)]
pub struct StubBrowserDriver;

#[async_trait]
impl BrowserDriver for StubBrowserDriver {}

/// The real browser driver (C2 #11 R1 / #13 R3): launches an isolated decoy
/// Chromium profile and runs a persona-paced session against a target URL,
/// enforcing the strict-separation guardrail at launch and the auth-flow
/// blocklist at navigation. A throwaway wrapper over [`DecoyBrowser`]: each
/// session launches, drives, and tears the browser down cleanly so no orphan
/// process is left behind.
#[derive(Debug, Clone)]
pub struct DecoyBrowserDriver {
    /// Where the session navigates (the URL whose Topics the persona accrues).
    target_url: String,
    /// Seed mixed into the persona-derived browsing cadence.
    seed: u64,
    /// Launch configuration (executable, isolation roots, headless).
    config: BrowserLaunchConfig,
}

impl DecoyBrowserDriver {
    /// A driver that navigates each session to `target_url` with the default
    /// (system Chromium, headless) launch config.
    pub fn new(target_url: impl Into<String>, seed: u64) -> Self {
        Self {
            target_url: target_url.into(),
            seed,
            config: BrowserLaunchConfig::new(),
        }
    }

    /// Override the launch configuration.
    pub fn with_config(mut self, config: BrowserLaunchConfig) -> Self {
        self.config = config;
        self
    }

    /// Run a persona-paced session for `persona` against the target URL on an
    /// isolated decoy profile keyed by the persona id, tearing down cleanly.
    pub async fn run_persona_session(&self, persona: &SyntheticPersona) -> Result<()> {
        let browser = DecoyBrowser::launch_with(&persona.id, self.config.clone()).await?;
        let result = self.drive(&browser, persona).await;
        // Always tear down, even on error, so no orphan browser survives.
        let close = browser.close().await;
        result.and(close)
    }

    /// Inner drive step: open a page, navigate (guardrail-checked), and run the
    /// persona's browsing cadence.
    async fn drive(&self, browser: &DecoyBrowser, persona: &SyntheticPersona) -> Result<()> {
        let page = browser.new_page().await?;
        page.navigate(&self.target_url).await?;
        page.browse_with_persona(persona, self.seed).await?;
        Ok(())
    }
}

#[async_trait]
impl BrowserDriver for DecoyBrowserDriver {
    /// Not supported by id alone: the real driver needs the persona record (for
    /// the derived cadence), so callers use
    /// [`DecoyBrowserDriver::run_persona_session`]. Returns
    /// [`CoreError::Unimplemented`] to signal the id-only entry point is not the
    /// one to use for the real driver.
    async fn run_session(&self, _persona_id: &str) -> Result<()> {
        Err(CoreError::Unimplemented(
            "DecoyBrowserDriver needs the persona record; use run_persona_session",
        ))
    }
}
