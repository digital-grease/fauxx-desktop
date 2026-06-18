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

//! Data Subject Access Request (DSAR) helper (C3 #16, D2c).
//!
//! Part of the lawful "deterministic-channel defense" the phone cannot do: the
//! desktop drafts the statutory privacy letters a data subject is entitled to
//! send, computes the legal response deadline, and tracks each request through
//! its lifecycle so an overdue controller can be chased.
//!
//! ## What this is and is NOT
//!
//! - It GENERATES and TRACKS letters; it never auto-sends them. Legal letters
//!   go out under the user's own hand (their real name and contact details), so
//!   the helper exports the rendered text for manual sending and records the
//!   send date the user reports back. There is no SMTP, no network, no
//!   reqwest.
//! - The deadline is statutory, computed from the send date: GDPR is one
//!   CALENDAR month (same day-of-month next month, clamped to the last day for
//!   short months, e.g. Jan 31 -> Feb 28/29); CCPA is a flat 45 DAYS. See
//!   [`RequestKind::deadline_from`].
//!
//! ## Layout
//!
//! - [`RequestKind`]: the four supported letter kinds (GDPR access / erasure,
//!   CCPA access / deletion) and their legal framing + deadline rule.
//! - [`Controller`]: the target data controller, either a known broker (reusing
//!   the [`crate::brokers`] registry) or an arbitrary name + contact.
//! - [`letter`]: pure letter rendering from templates filled with persona /
//!   subject details and the controller.
//! - [`DsarRequest`]: the persisted lifecycle record (drafted, sent,
//!   acknowledged, fulfilled), the computed deadline, and the overdue /
//!   due-soon predicates.
//!
//! The async Core surface ([`crate::Core`]) exposes generate / record / list /
//! track / export and the overdue query.

pub mod letter;

use serde::{Deserialize, Serialize};
use time::{Date, Month, OffsetDateTime};

use crate::brokers::{self, BrokerTemplate};
use crate::error::{CoreError, Result};

pub use letter::{DsarLetter, SubjectDetails};

/// Milliseconds in one day, for converting between epoch-millis timestamps and
/// the `time` date math used for statutory deadlines.
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// The flat statutory window for a CCPA request, in days.
const CCPA_DEADLINE_DAYS: i64 = 45;

/// The four supported statutory request kinds. Each carries its legal framing
/// (cited regulation, right invoked) and its deadline rule.
///
/// GDPR (EU/UK) grants a right of access (Art. 15) and a right to erasure
/// (Art. 17), answerable within one calendar month (Art. 12(3)). CCPA/CPRA
/// (California) grants a right to know and a right to delete, answerable within
/// 45 days (Cal. Civ. Code 1798.130(a)(2)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestKind {
    /// GDPR right of access (Art. 15): a copy of the personal data held.
    GdprAccess,
    /// GDPR right to erasure / "right to be forgotten" (Art. 17): deletion.
    GdprDeletion,
    /// CCPA/CPRA right to know: the categories and specifics of data collected.
    CcpaAccess,
    /// CCPA/CPRA right to delete: deletion of personal information.
    CcpaDeletion,
}

impl RequestKind {
    /// Every request kind, in declaration order (for listing in the GUI/CLI).
    pub const ALL: &'static [RequestKind] = &[
        RequestKind::GdprAccess,
        RequestKind::GdprDeletion,
        RequestKind::CcpaAccess,
        RequestKind::CcpaDeletion,
    ];

    /// The stable persistence/wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestKind::GdprAccess => "gdpr-access",
            RequestKind::GdprDeletion => "gdpr-deletion",
            RequestKind::CcpaAccess => "ccpa-access",
            RequestKind::CcpaDeletion => "ccpa-deletion",
        }
    }

    /// Parse the persisted string form, failing closed on an unknown value.
    pub fn from_str_strict(s: &str) -> Result<Self> {
        match s {
            "gdpr-access" => Ok(RequestKind::GdprAccess),
            "gdpr-deletion" => Ok(RequestKind::GdprDeletion),
            "ccpa-access" => Ok(RequestKind::CcpaAccess),
            "ccpa-deletion" => Ok(RequestKind::CcpaDeletion),
            other => Err(CoreError::Dsar(format!("unknown request kind {other:?}"))),
        }
    }

    /// Whether this is a GDPR (EU/UK) request, as opposed to CCPA (California).
    /// Drives the deadline rule and the cited regulation.
    pub fn is_gdpr(&self) -> bool {
        matches!(self, RequestKind::GdprAccess | RequestKind::GdprDeletion)
    }

    /// Whether this is a deletion/erasure request (as opposed to an access one).
    pub fn is_deletion(&self) -> bool {
        matches!(self, RequestKind::GdprDeletion | RequestKind::CcpaDeletion)
    }

    /// The regulation cited in the letter ("GDPR" / "CCPA/CPRA").
    pub fn regulation(&self) -> &'static str {
        if self.is_gdpr() {
            "GDPR"
        } else {
            "CCPA/CPRA"
        }
    }

    /// The specific legal article/section the letter invokes.
    pub fn legal_basis(&self) -> &'static str {
        match self {
            RequestKind::GdprAccess => "Article 15 of the EU/UK GDPR (right of access)",
            RequestKind::GdprDeletion => "Article 17 of the EU/UK GDPR (right to erasure)",
            RequestKind::CcpaAccess => {
                "the California Consumer Privacy Act (CCPA/CPRA) right to know, \
                 Cal. Civ. Code 1798.110 and 1798.115"
            }
            RequestKind::CcpaDeletion => {
                "the California Consumer Privacy Act (CCPA/CPRA) right to delete, \
                 Cal. Civ. Code 1798.105"
            }
        }
    }

    /// A short human-readable label for lists/menus.
    pub fn label(&self) -> &'static str {
        match self {
            RequestKind::GdprAccess => "GDPR access request",
            RequestKind::GdprDeletion => "GDPR erasure request",
            RequestKind::CcpaAccess => "CCPA right-to-know request",
            RequestKind::CcpaDeletion => "CCPA deletion request",
        }
    }

    /// Compute the statutory response deadline (epoch millis) from the send date
    /// (epoch millis), per this kind's rule:
    ///
    /// - GDPR: one CALENDAR month. The deadline is the same day-of-month in the
    ///   next month, clamped to that month's last day when it is shorter (so a
    ///   send on Jan 31 yields Feb 28, or Feb 29 in a leap year). The clock time
    ///   of day is preserved.
    /// - CCPA: a flat 45 days after the send date.
    ///
    /// Fails closed if the send timestamp cannot be interpreted as a valid date
    /// (which cannot happen for a sane epoch-millis value).
    pub fn deadline_from(&self, sent_at_millis: i64) -> Result<i64> {
        if self.is_gdpr() {
            add_one_calendar_month(sent_at_millis)
        } else {
            Ok(sent_at_millis + CCPA_DEADLINE_DAYS * MILLIS_PER_DAY)
        }
    }
}

/// Add exactly one calendar month to an epoch-millis timestamp, clamping the
/// day-of-month to the target month's length (Jan 31 + 1 month = Feb 28/29) and
/// preserving the time-of-day. This is the GDPR "one month" rule.
fn add_one_calendar_month(millis: i64) -> Result<i64> {
    let odt = OffsetDateTime::from_unix_timestamp_nanos((millis as i128) * 1_000_000)
        .map_err(|e| CoreError::Dsar(format!("invalid send timestamp: {e}")))?;
    let date = odt.date();
    let (year, next_month) = next_month_and_year(date.year(), date.month());
    let max_day = days_in_month(year, next_month);
    let day = date.day().min(max_day);
    let new_date = Date::from_calendar_date(year, next_month, day)
        .map_err(|e| CoreError::Dsar(format!("could not build deadline date: {e}")))?;
    let new_odt = odt.replace_date(new_date);
    Ok((new_odt.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// The month and year one calendar month after `(year, month)`, rolling the
/// year over at December.
fn next_month_and_year(year: i32, month: Month) -> (i32, Month) {
    match month {
        Month::December => (year + 1, Month::January),
        other => (year, other.next()),
    }
}

/// The number of days in `month` of `year` (handles leap Februaries).
fn days_in_month(year: i32, month: Month) -> u8 {
    month.length(year)
}

/// The target data controller a DSAR letter is addressed to.
///
/// Either a KNOWN broker (reusing the [`crate::brokers`] registry, so the
/// display name and a contact are pulled from the bundled template) or an
/// ARBITRARY controller the user names with their own contact string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    /// The broker id this controller corresponds to in the [`crate::brokers`]
    /// registry, or `None` for an arbitrary controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_id: Option<String>,
    /// The controller's display name (e.g. "Spokeo" or "Example Corp").
    pub name: String,
    /// A contact line for the controller: a privacy email address, a web form
    /// URL, or a postal address. May be empty if the user has none yet.
    #[serde(default)]
    pub contact: String,
}

impl Controller {
    /// An arbitrary controller named directly by the user.
    pub fn arbitrary(name: impl Into<String>, contact: impl Into<String>) -> Self {
        Self {
            broker_id: None,
            name: name.into(),
            contact: contact.into(),
        }
    }

    /// A controller built from a known broker template: the display name and a
    /// best contact (the email address for email-method brokers, else the
    /// opt-out URL) are taken from the registry.
    pub fn from_broker(broker_id: &str, template: &BrokerTemplate) -> Self {
        let contact = template
            .email_to
            .clone()
            .unwrap_or_else(|| template.opt_out_url.clone());
        Self {
            broker_id: Some(broker_id.to_string()),
            name: template.display_name.clone(),
            contact,
        }
    }

    /// Resolve a controller by broker id from the bundled registry. Returns
    /// [`CoreError::NotFound`] if the id is unknown.
    pub fn resolve_broker(broker_id: &str) -> Result<Self> {
        let template = brokers::broker(broker_id)?;
        Ok(Self::from_broker(broker_id, template))
    }
}

/// The lifecycle status of a DSAR request.
///
/// `drafted -> sent -> acknowledged -> fulfilled` is the happy path. `overdue`
/// is not a stored status but a derived condition (see
/// [`DsarRequest::is_overdue`]); a request stays in its stored status while the
/// overdue predicate flags that its deadline has lapsed without fulfillment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    /// The letter has been generated but not yet sent.
    Drafted,
    /// The user has sent the letter; the statutory clock is running.
    Sent,
    /// The controller acknowledged receipt (the clock is still running).
    Acknowledged,
    /// The controller fulfilled the request (settled; no longer overdue-able).
    Fulfilled,
}

impl RequestStatus {
    /// The stored string form (matches the serde representation).
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestStatus::Drafted => "drafted",
            RequestStatus::Sent => "sent",
            RequestStatus::Acknowledged => "acknowledged",
            RequestStatus::Fulfilled => "fulfilled",
        }
    }

    /// Parse the stored string form back into a status.
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "drafted" => Some(RequestStatus::Drafted),
            "sent" => Some(RequestStatus::Sent),
            "acknowledged" => Some(RequestStatus::Acknowledged),
            "fulfilled" => Some(RequestStatus::Fulfilled),
            _ => None,
        }
    }

    /// Whether the statutory clock is running: the request has been sent and the
    /// controller has not yet fulfilled it. A `drafted` request has no deadline
    /// running yet; a `fulfilled` one is settled.
    pub fn clock_running(&self) -> bool {
        matches!(self, RequestStatus::Sent | RequestStatus::Acknowledged)
    }
}

/// A persisted DSAR request record (C3 #16).
///
/// One row per letter sent to one controller for one subject. Tracked through
/// its lifecycle; the [`deadline`](Self::deadline) is the statutory response
/// date computed from [`sent_at`](Self::sent_at) per the [`kind`](Self::kind)'s
/// rule. Persisted in the `dsar_requests` table; the
/// [`crate::store::EncryptedStore`] round-trips this whole record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DsarRequest {
    /// Stable id for this request (UUID v4 string).
    pub id: String,
    /// The statutory request kind (GDPR/CCPA, access/deletion).
    pub kind: RequestKind,
    /// The persona id this request is filed on behalf of (the data subject).
    pub persona_id: String,
    /// The target data controller.
    pub controller: Controller,
    /// Current lifecycle status.
    pub status: RequestStatus,
    /// Epoch millis the request was created (drafted).
    pub created_at: i64,
    /// Epoch millis the letter was sent, once the user reports it. `None` while
    /// still `drafted` (the statutory clock has not started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<i64>,
    /// Epoch millis the statutory deadline falls, computed from `sent_at`.
    /// `None` while still `drafted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<i64>,
}

impl DsarRequest {
    /// Build a new request stamped `drafted`. No send date or deadline yet: the
    /// statutory clock starts only when the letter is marked sent (see
    /// [`DsarRequest::mark_sent`]).
    pub fn draft(
        id: String,
        kind: RequestKind,
        persona_id: &str,
        controller: Controller,
        created_at: i64,
    ) -> Self {
        Self {
            id,
            kind,
            persona_id: persona_id.to_string(),
            controller,
            status: RequestStatus::Drafted,
            created_at,
            sent_at: None,
            deadline: None,
        }
    }

    /// Mark this request `sent` as of `sent_at`, computing and storing the
    /// statutory deadline per the kind's rule. Idempotent re-send updates both.
    pub fn mark_sent(&mut self, sent_at: i64) -> Result<()> {
        self.status = RequestStatus::Sent;
        self.sent_at = Some(sent_at);
        self.deadline = Some(self.kind.deadline_from(sent_at)?);
        Ok(())
    }

    /// Mark this request `acknowledged` (the controller confirmed receipt). The
    /// deadline keeps running.
    pub fn mark_acknowledged(&mut self) {
        self.status = RequestStatus::Acknowledged;
    }

    /// Mark this request `fulfilled` (settled). It can no longer be overdue.
    pub fn mark_fulfilled(&mut self) {
        self.status = RequestStatus::Fulfilled;
    }

    /// Whether this request is OVERDUE as of `now`: the statutory clock is
    /// running (sent/acknowledged, not yet fulfilled) and the deadline has
    /// passed. A drafted or fulfilled request is never overdue.
    pub fn is_overdue(&self, now: i64) -> bool {
        match self.deadline {
            Some(deadline) => self.status.clock_running() && now >= deadline,
            None => false,
        }
    }

    /// Whether this request is DUE SOON as of `now`: the clock is running and
    /// the deadline is within `window_millis` ahead (but not yet passed). Used
    /// to surface "respond soon" reminders before the deadline lapses.
    pub fn is_due_soon(&self, now: i64, window_millis: i64) -> bool {
        match self.deadline {
            Some(deadline) => {
                self.status.clock_running() && now < deadline && deadline - now <= window_millis
            }
            None => false,
        }
    }

    /// Days remaining until the deadline as of `now` (negative when overdue), or
    /// `None` while no deadline is set (still drafted). Truncates toward zero;
    /// purely for display.
    pub fn days_remaining(&self, now: i64) -> Option<i64> {
        self.deadline.map(|d| (d - now) / MILLIS_PER_DAY)
    }

    /// Render this request's letter text for manual sending / export. Pure;
    /// does NOT send. The `subject` supplies the real identity details the user
    /// signs the letter with.
    pub fn render_letter(&self, subject: &SubjectDetails) -> DsarLetter {
        letter::render(self, subject)
    }
}

/// One day, in millis, exposed for callers that want a default "due soon"
/// window unit without re-deriving the constant.
pub const ONE_DAY_MILLIS: i64 = MILLIS_PER_DAY;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_string_round_trips() -> Result<()> {
        for kind in RequestKind::ALL {
            assert_eq!(RequestKind::from_str_strict(kind.as_str())?, *kind);
        }
        assert!(matches!(
            RequestKind::from_str_strict("garbage"),
            Err(CoreError::Dsar(_))
        ));
        Ok(())
    }

    #[test]
    fn status_string_round_trips() {
        for s in [
            RequestStatus::Drafted,
            RequestStatus::Sent,
            RequestStatus::Acknowledged,
            RequestStatus::Fulfilled,
        ] {
            assert_eq!(RequestStatus::parse_str(s.as_str()), Some(s));
        }
        assert_eq!(RequestStatus::parse_str("nope"), None);
    }

    /// Build an epoch-millis timestamp from a UTC calendar date at midnight.
    fn at(year: i32, month: Month, day: u8) -> Result<i64> {
        let date = Date::from_calendar_date(year, month, day)
            .map_err(|e| CoreError::Dsar(format!("test date: {e}")))?;
        let odt = date.midnight().assume_utc();
        Ok((odt.unix_timestamp_nanos() / 1_000_000) as i64)
    }

    /// Read an epoch-millis timestamp back into a (year, month, day) UTC date.
    fn date_of(millis: i64) -> Result<(i32, Month, u8)> {
        let odt = OffsetDateTime::from_unix_timestamp_nanos((millis as i128) * 1_000_000)
            .map_err(|e| CoreError::Dsar(format!("test timestamp: {e}")))?;
        let d = odt.date();
        Ok((d.year(), d.month(), d.day()))
    }

    #[test]
    fn gdpr_deadline_is_one_calendar_month_same_day() -> Result<()> {
        // A mid-month send: the deadline is the same day next month.
        let sent = at(2026, Month::March, 15)?;
        let deadline = RequestKind::GdprAccess.deadline_from(sent)?;
        assert_eq!(date_of(deadline)?, (2026, Month::April, 15));
        Ok(())
    }

    #[test]
    fn gdpr_deadline_clamps_jan_31_to_end_of_february() -> Result<()> {
        // The named edge case: Jan 31 + one month clamps to Feb 28 (non-leap).
        let sent = at(2026, Month::January, 31)?;
        let deadline = RequestKind::GdprDeletion.deadline_from(sent)?;
        assert_eq!(date_of(deadline)?, (2026, Month::February, 28));
        Ok(())
    }

    #[test]
    fn gdpr_deadline_clamps_jan_31_to_feb_29_in_leap_year() -> Result<()> {
        // 2028 is a leap year: Jan 31 + one month clamps to Feb 29, not Feb 28.
        let sent = at(2028, Month::January, 31)?;
        let deadline = RequestKind::GdprAccess.deadline_from(sent)?;
        assert_eq!(date_of(deadline)?, (2028, Month::February, 29));
        Ok(())
    }

    #[test]
    fn gdpr_deadline_rolls_year_over_at_december() -> Result<()> {
        let sent = at(2026, Month::December, 10)?;
        let deadline = RequestKind::GdprDeletion.deadline_from(sent)?;
        assert_eq!(date_of(deadline)?, (2027, Month::January, 10));
        Ok(())
    }

    #[test]
    fn gdpr_deadline_preserves_time_of_day() -> Result<()> {
        // Mid-day send keeps its clock time across the month roll.
        let date = Date::from_calendar_date(2026, Month::June, 12)
            .map_err(|e| CoreError::Dsar(format!("test date: {e}")))?;
        let odt = date
            .with_hms(13, 45, 0)
            .map_err(|e| CoreError::Dsar(format!("test time: {e}")))?
            .assume_utc();
        let sent = (odt.unix_timestamp_nanos() / 1_000_000) as i64;
        let deadline = RequestKind::GdprAccess.deadline_from(sent)?;
        let back = OffsetDateTime::from_unix_timestamp_nanos((deadline as i128) * 1_000_000)
            .map_err(|e| CoreError::Dsar(format!("test timestamp: {e}")))?;
        assert_eq!(back.month(), Month::July);
        assert_eq!(back.day(), 12);
        assert_eq!((back.hour(), back.minute()), (13, 45));
        Ok(())
    }

    #[test]
    fn ccpa_deadline_is_flat_45_days() -> Result<()> {
        let sent = at(2026, Month::January, 31)?;
        let deadline = RequestKind::CcpaAccess.deadline_from(sent)?;
        // 45 days, NOT a calendar month: Jan 31 + 45d = Mar 17 (2026).
        assert_eq!(deadline, sent + 45 * MILLIS_PER_DAY);
        assert_eq!(date_of(deadline)?, (2026, Month::March, 17));
        Ok(())
    }

    #[test]
    fn gdpr_and_ccpa_deadlines_differ_for_the_same_send() -> Result<()> {
        let sent = at(2026, Month::January, 15)?;
        let gdpr = RequestKind::GdprAccess.deadline_from(sent)?;
        let ccpa = RequestKind::CcpaAccess.deadline_from(sent)?;
        // GDPR one month = Feb 15; CCPA 45 days = Mar 1. CCPA is later here.
        assert_eq!(date_of(gdpr)?, (2026, Month::February, 15));
        assert_eq!(date_of(ccpa)?, (2026, Month::March, 1));
        assert!(ccpa > gdpr);
        Ok(())
    }

    #[test]
    fn lifecycle_drafted_has_no_deadline_then_sent_computes_it() -> Result<()> {
        let sent = at(2026, Month::March, 1)?;
        let mut req = DsarRequest::draft(
            "r1".to_string(),
            RequestKind::GdprAccess,
            "p1",
            Controller::arbitrary("Example Corp", "privacy@example.test"),
            sent - ONE_DAY_MILLIS,
        );
        // Drafted: no clock, never overdue.
        assert_eq!(req.status, RequestStatus::Drafted);
        assert!(req.deadline.is_none());
        assert!(!req.is_overdue(sent + 100 * ONE_DAY_MILLIS));
        assert!(req.days_remaining(sent).is_none());

        // Sent: deadline computed, clock running.
        req.mark_sent(sent)?;
        assert_eq!(req.status, RequestStatus::Sent);
        let deadline = req
            .deadline
            .ok_or_else(|| CoreError::Dsar("deadline set after send".into()))?;
        assert_eq!(date_of(deadline)?, (2026, Month::April, 1));
        assert!(!req.is_overdue(sent));
        assert!(req.is_overdue(deadline + 1));
        assert_eq!(req.days_remaining(sent), Some(31));
        Ok(())
    }

    #[test]
    fn overdue_predicate_respects_status() -> Result<()> {
        let sent = at(2026, Month::March, 1)?;
        let mut req = DsarRequest::draft(
            "r2".to_string(),
            RequestKind::CcpaDeletion,
            "p1",
            Controller::arbitrary("Example Corp", ""),
            sent,
        );
        req.mark_sent(sent)?;
        let deadline = req
            .deadline
            .ok_or_else(|| CoreError::Dsar("deadline".into()))?;

        // Sent + past deadline: overdue.
        assert!(req.is_overdue(deadline + ONE_DAY_MILLIS));
        // Acknowledged keeps the clock running: still overdue.
        req.mark_acknowledged();
        assert!(req.is_overdue(deadline + ONE_DAY_MILLIS));
        // Fulfilled settles it: never overdue, even past the deadline.
        req.mark_fulfilled();
        assert!(!req.is_overdue(deadline + 100 * ONE_DAY_MILLIS));
        Ok(())
    }

    #[test]
    fn due_soon_predicate_fires_inside_window_only() -> Result<()> {
        let sent = at(2026, Month::March, 1)?;
        let mut req = DsarRequest::draft(
            "r3".to_string(),
            RequestKind::GdprAccess,
            "p1",
            Controller::arbitrary("Example Corp", ""),
            sent,
        );
        req.mark_sent(sent)?;
        let deadline = req
            .deadline
            .ok_or_else(|| CoreError::Dsar("deadline".into()))?;
        let window = 7 * ONE_DAY_MILLIS;

        // Far before the deadline: not due soon.
        assert!(!req.is_due_soon(deadline - 30 * ONE_DAY_MILLIS, window));
        // Within the window: due soon.
        assert!(req.is_due_soon(deadline - 3 * ONE_DAY_MILLIS, window));
        // Past the deadline: overdue, not "due soon".
        assert!(!req.is_due_soon(deadline + ONE_DAY_MILLIS, window));
        assert!(req.is_overdue(deadline + ONE_DAY_MILLIS));
        Ok(())
    }

    #[test]
    fn controller_from_broker_pulls_name_and_contact() -> Result<()> {
        // A web-form broker uses its opt-out URL as the contact.
        let spokeo = Controller::resolve_broker("spokeo")?;
        assert_eq!(spokeo.broker_id.as_deref(), Some("spokeo"));
        assert_eq!(spokeo.name, "Spokeo");
        assert!(spokeo.contact.starts_with("https://"));

        // An email-method broker uses its privacy email as the contact.
        let mylife = Controller::resolve_broker("mylife")?;
        assert!(mylife.contact.contains('@'));

        assert!(matches!(
            Controller::resolve_broker("no-such-broker"),
            Err(CoreError::NotFound(_))
        ));
        Ok(())
    }
}
