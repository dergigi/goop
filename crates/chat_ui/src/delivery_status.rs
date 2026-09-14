use chat::SendReport;

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
