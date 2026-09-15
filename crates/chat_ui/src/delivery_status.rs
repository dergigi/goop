use chat::SendReport;

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
        (true, _, true) => "Partially sent · paused",
        (true, true, _) => "Partially sent · queued",
        (true, false, false) => "Accepted by relay",
        (false, _, true) => "Paused",
        (false, true, _) => "Queued for retry",
        _ if report.failed() => "Failed",
        _ => "Waiting for delivery status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;

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
        assert_eq!(delivery_status_label(&report), "Partially sent · paused");
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
        assert_eq!(delivery_status_label(&report), "Paused");
    }
}
