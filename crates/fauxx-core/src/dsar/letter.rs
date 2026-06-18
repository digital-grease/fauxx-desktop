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

//! DSAR letter rendering (C3 #16, D2c).
//!
//! Pure, hermetic letter generation: a [`DsarRequest`] plus the data subject's
//! real identity details are filled into a per-kind template, producing the
//! exported letter text for MANUAL sending. Nothing here sends, persists, or
//! touches the network.
//!
//! A DSAR is sent under the subject's OWN identity (their real legal name and
//! contact), not the synthetic persona's: the controller must be able to match
//! the request to the records it holds. The persona id is carried on the
//! request only for the user's own bookkeeping (which decoy this opt-out
//! belongs to). [`SubjectDetails`] therefore carries the real-world details the
//! letter is signed with.

use serde::{Deserialize, Serialize};

use super::{Controller, DsarRequest, RequestKind};

/// The real-world identity details a DSAR is sent under.
///
/// These are the user's OWN details (legal name, a reply-to contact, and any
/// account identifiers that help the controller find the records), supplied at
/// export time. They are not persisted with the request; the caller passes them
/// when rendering so they never sit in the store with the lifecycle record.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectDetails {
    /// The data subject's full legal name (signs the letter).
    pub full_name: String,
    /// A reply-to contact line (postal address or email) the controller
    /// responds to. May be multi-line.
    #[serde(default)]
    pub reply_to: String,
    /// Optional account identifiers / email addresses on file with the
    /// controller that help it locate the records (e.g. "account email:
    /// alice@example.com"). One per line.
    #[serde(default)]
    pub identifiers: Vec<String>,
}

impl SubjectDetails {
    /// A minimal subject with just a legal name.
    pub fn new(full_name: impl Into<String>) -> Self {
        Self {
            full_name: full_name.into(),
            reply_to: String::new(),
            identifiers: Vec::new(),
        }
    }

    /// Set the reply-to contact line (builder style).
    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = reply_to.into();
        self
    }

    /// Add an identifier line that helps the controller locate the records.
    pub fn with_identifier(mut self, identifier: impl Into<String>) -> Self {
        self.identifiers.push(identifier.into());
        self
    }
}

/// A rendered DSAR letter ready for the user to review and send by hand.
///
/// Carries both a one-line `subject` (for an email subject line) and the full
/// `body` text. The Core export API returns this; the GUI/CLI display it and
/// the user copies it into their own mail client. Nothing auto-sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DsarLetter {
    /// The request id this letter renders, for cross-reference.
    pub request_id: String,
    /// A subject line suitable for an email.
    pub subject: String,
    /// The full letter body text (plain text, ready to send).
    pub body: String,
}

/// Render the letter for `request`, signed with `subject`. Pure; never sends.
pub fn render(request: &DsarRequest, subject: &SubjectDetails) -> DsarLetter {
    let kind = request.kind;
    let controller = &request.controller;

    let subject_line = subject_line(kind, controller);
    let body = body(kind, controller, subject);

    DsarLetter {
        request_id: request.id.clone(),
        subject: subject_line,
        body,
    }
}

/// The email subject line for a request of this kind to this controller.
fn subject_line(kind: RequestKind, controller: &Controller) -> String {
    let action = match kind {
        RequestKind::GdprAccess => "Data Subject Access Request (GDPR Article 15)",
        RequestKind::GdprDeletion => "Erasure Request (GDPR Article 17)",
        RequestKind::CcpaAccess => "Request to Know (CCPA/CPRA)",
        RequestKind::CcpaDeletion => "Request to Delete (CCPA/CPRA)",
    };
    format!("{action} - {}", controller.name)
}

/// The full letter body, filled from the kind, controller, and subject.
fn body(kind: RequestKind, controller: &Controller, subject: &SubjectDetails) -> String {
    let name = if subject.full_name.trim().is_empty() {
        "[YOUR FULL LEGAL NAME]"
    } else {
        subject.full_name.trim()
    };

    let mut out = String::new();

    // Salutation and addressing.
    out.push_str("To the Data Protection Officer / Privacy Team,\n");
    out.push_str(&format!("{}\n", controller.name));
    if !controller.contact.trim().is_empty() {
        out.push_str(&format!("{}\n", controller.contact.trim()));
    }
    out.push('\n');

    // Opening sentence stating the right and the regulation.
    out.push_str(&opening(kind));
    out.push_str("\n\n");

    // Identification block, so the controller can match the records.
    out.push_str(&format!("I am the data subject named below:\n  {name}\n"));
    if !subject.reply_to.trim().is_empty() {
        out.push_str(&format!(
            "Reply-to / contact:\n  {}\n",
            indent_lines(subject.reply_to.trim())
        ));
    }
    if !subject.identifiers.is_empty() {
        out.push_str("Identifiers that may help you locate my records:\n");
        for id in &subject.identifiers {
            if !id.trim().is_empty() {
                out.push_str(&format!("  - {}\n", id.trim()));
            }
        }
    }
    out.push('\n');

    // The substantive request and the statutory deadline.
    out.push_str(request_paragraph(kind));
    out.push_str("\n\n");
    out.push_str(deadline_sentence(kind));
    out.push_str("\n\n");

    // Closing.
    out.push_str(
        "Please confirm receipt of this request and let me know if you require anything \
         further to verify my identity. I expect no fee for this request.\n\n",
    );
    out.push_str("Yours faithfully,\n");
    out.push_str(name);
    out.push('\n');

    out
}

/// The opening sentence naming the right invoked and the regulation cited.
fn opening(kind: RequestKind) -> String {
    format!(
        "I am writing to exercise my rights under {}. This is a formal request and I ask \
         that you treat it as such.",
        kind.legal_basis()
    )
}

/// The substantive request paragraph for this kind.
fn request_paragraph(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::GdprAccess => {
            "Under Article 15 of the GDPR, I request confirmation of whether you are \
             processing personal data concerning me and, if so, a copy of that personal \
             data together with the supplementary information required by Article 15(1): \
             the purposes of processing, the categories of data, the recipients or \
             categories of recipient, the retention period, the source of the data, and \
             whether any automated decision-making (including profiling) is applied."
        }
        RequestKind::GdprDeletion => {
            "Under Article 17 of the GDPR, I request that you erase all personal data you \
             hold concerning me without undue delay, and that you inform any third parties \
             and processors to whom the data has been disclosed of this erasure request. \
             Please confirm in writing once the erasure is complete and identify any data \
             you are retaining, citing the specific legal ground for doing so."
        }
        RequestKind::CcpaAccess => {
            "Under the CCPA/CPRA, I request that you disclose the specific pieces of personal \
             information you have collected about me, the categories of personal information \
             collected, the categories of sources, the business or commercial purpose for \
             collecting it, and the categories of third parties with whom you have shared or \
             sold it."
        }
        RequestKind::CcpaDeletion => {
            "Under the CCPA/CPRA, I request that you delete all personal information you have \
             collected about me and direct any service providers to delete it as well. Please \
             confirm in writing once the deletion is complete and identify any information you \
             are retaining together with the statutory exemption you are relying on."
        }
    }
}

/// The sentence stating the statutory response window for this kind.
fn deadline_sentence(kind: RequestKind) -> &'static str {
    if kind.is_gdpr() {
        "Article 12(3) of the GDPR requires that you respond without undue delay and in any \
         event within one month of receipt of this request."
    } else {
        "The CCPA/CPRA requires that you respond to this verifiable consumer request within \
         45 days of receipt."
    }
}

/// Indent every line after the first by two spaces, so a multi-line reply-to
/// block lines up under its label.
fn indent_lines(text: &str) -> String {
    text.replace('\n', "\n  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsar::{Controller, DsarRequest, RequestKind};

    fn request(kind: RequestKind) -> DsarRequest {
        DsarRequest::draft(
            "req-abc".to_string(),
            kind,
            "persona-1",
            Controller::arbitrary("Example Corp", "privacy@example.test"),
            1_700_000_000_000,
        )
    }

    fn subject() -> SubjectDetails {
        SubjectDetails::new("Alex Subject")
            .with_reply_to("123 Main St\nAnytown")
            .with_identifier("account email: alex@example.test")
    }

    #[test]
    fn gdpr_access_letter_carries_article_15_and_one_month() {
        let letter = render(&request(RequestKind::GdprAccess), &subject());
        assert_eq!(letter.request_id, "req-abc");
        assert!(letter.subject.contains("Article 15"));
        assert!(letter.subject.contains("Example Corp"));
        // GDPR legal framing.
        assert!(letter.body.contains("Article 15 of the GDPR"));
        assert!(letter.body.contains("GDPR"));
        // GDPR deadline framing, NOT the CCPA 45-day language.
        assert!(letter.body.contains("within one month"));
        assert!(!letter.body.contains("45 days"));
        // Subject details are filled in.
        assert!(letter.body.contains("Alex Subject"));
        assert!(letter.body.contains("123 Main St"));
        assert!(letter.body.contains("alex@example.test"));
        // Controller is addressed.
        assert!(letter.body.contains("Example Corp"));
        assert!(letter.body.contains("privacy@example.test"));
    }

    #[test]
    fn gdpr_deletion_letter_invokes_article_17() {
        let letter = render(&request(RequestKind::GdprDeletion), &subject());
        assert!(letter.subject.contains("Article 17"));
        assert!(letter.body.contains("Article 17 of the GDPR"));
        assert!(letter.body.contains("erase"));
        assert!(letter.body.contains("within one month"));
    }

    #[test]
    fn ccpa_access_letter_carries_ccpa_and_45_days() {
        let letter = render(&request(RequestKind::CcpaAccess), &subject());
        assert!(letter.subject.contains("CCPA/CPRA"));
        assert!(letter.body.contains("CCPA/CPRA"));
        // CCPA deadline framing, NOT the GDPR one-month language.
        assert!(letter.body.contains("within 45 days"));
        assert!(!letter.body.contains("within one month"));
    }

    #[test]
    fn ccpa_deletion_letter_requests_deletion() {
        let letter = render(&request(RequestKind::CcpaDeletion), &subject());
        assert!(letter.subject.contains("Request to Delete"));
        assert!(letter.body.contains("delete all personal information"));
        assert!(letter.body.contains("within 45 days"));
    }

    #[test]
    fn empty_subject_name_uses_a_placeholder() {
        let letter = render(
            &request(RequestKind::GdprAccess),
            &SubjectDetails::default(),
        );
        assert!(letter.body.contains("[YOUR FULL LEGAL NAME]"));
    }
}
