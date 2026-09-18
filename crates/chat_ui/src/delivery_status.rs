use gpui::{Animation, AnimationExt, IntoElement, Transformation, percentage};
use ui::{Icon, IconName, Sizable};

use chat::SendReport;

/// Hold still, then turn halfway around. The symmetric outline avoids a jump
/// when the three-second animation repeats.
pub(super) fn queued_hourglass() -> impl IntoElement {
    Icon::new(IconName::Hourglass).xsmall().with_animation(
        "queued-hourglass",
        Animation::new(std::time::Duration::from_secs(3)).repeat(),
        |icon, progress| {
            let turn = ((progress - 0.85) / 0.15).clamp(0.0, 1.0);
            let eased = turn * turn * (3.0 - 2.0 * turn);
            icon.transform(Transformation::rotate(percentage(eased * 0.5)))
        },
    )
}

/// Relay acknowledgement only; this is not a read or device receipt.
pub(super) fn delivery_checks(reports: &[SendReport]) -> usize {
    let recipients: Vec<_> = reports.iter().filter(|report| !report.self_copy).collect();
    if !recipients.is_empty() && recipients.iter().all(|report| report.success()) {
        2
    } else if reports.iter().any(|report| report.success()) {
        1
    } else { 0 }
}

pub(super) fn delivery_status_label(report: &SendReport) -> &'static str {
    match (report.success(), report.pending(), report.paused) {
        (true, _, true) => "Partially sent · waiting for retry",
        (true, true, _) => "Partially sent · queued",
        (true, false, false) => "Accepted by relay",
        (false, _, true) => "Waiting for retry",
        (false, true, _) => "Queued for retry",
        _ if report.failed() => "Failed",
        _ => "Waiting for delivery status",
    }
}

/// Show inline controls only for incomplete copies that need attention.
pub(super) fn needs_attention(reports: &[SendReport]) -> bool {
    reports.iter().any(|report| report.paused || (report.failed() && (report.pending() || !report.success())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;

    #[test]
    fn inline_retry_is_hidden_for_normal_queue_and_completed_delivery() {
        let mut report = SendReport::new(Keys::generate().public_key());
        report.queued = true;
        assert!(!needs_attention(&[report.clone()]));
        report.error = Some("Signer refused".into());
        assert!(needs_attention(&[report.clone()]));
        report.queued = false;
        report.error = None;
        report.paused = true;
        assert!(needs_attention(&[report.clone()]));
        report.paused = false;
        report.accepted = true;
        assert!(!needs_attention(&[report]));
    }

    #[test]
    fn double_check_requires_every_other_recipient_and_excludes_self_copy() {
        let mut self_copy = SendReport::new(Keys::generate().public_key());
        self_copy.self_copy = true;
        self_copy.accepted = true;
        let mut first = SendReport::new(Keys::generate().public_key());
        let mut second = SendReport::new(Keys::generate().public_key());
        assert_eq!(delivery_checks(&[]), 0);
        assert_eq!(delivery_checks(&[first.clone()]), 0);
        assert_eq!(delivery_checks(&[self_copy.clone(), first.clone()]), 1);
        first.accepted = true;
        assert_eq!(delivery_checks(&[first.clone(), second.clone()]), 1);
        second.accepted = true;
        self_copy.accepted = false;
        assert_eq!(delivery_checks(&[self_copy, first, second]), 2);
    }

    #[test]
    fn reports_progress_from_queued_through_partial_to_accepted() {
        let mut report = SendReport::new(Keys::generate().public_key());
        report.queued = true;
        assert_eq!(delivery_status_label(&report), "Queued for retry");
        report.accepted = true;
        assert_eq!(delivery_status_label(&report), "Partially sent · queued");
        report.queued = false;
        report.paused = true;
        assert_eq!(delivery_status_label(&report), "Partially sent · waiting for retry");
        report.paused = false;
        assert_eq!(delivery_status_label(&report), "Accepted by relay");
    }

    #[test]
    fn errors_distinguish_retrying_from_terminal_failure() {
        let mut report = SendReport::new(Keys::generate().public_key()).error("Offline");
        assert_eq!(delivery_status_label(&report), "Failed");
        report.queued = true;
        assert_eq!(delivery_status_label(&report), "Queued for retry");
        report.queued = false;
        report.paused = true;
        assert_eq!(delivery_status_label(&report), "Waiting for retry");
    }
}
