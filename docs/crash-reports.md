# Crash reports

Starting with Goop 2.10.0, Goop installs crash capture before initializing its window, fonts, signer, or database. A Rust panic or an unhandled Windows exception writes a minimal report in the app's support directory:

- Windows: `%LOCALAPPDATA%\goop\crashes`
- macOS: `~/Library/Application Support/Goop/crashes`
- Linux: the `crashes` folder under Goop's XDG data directory (inside its app data directory when sandboxed).

On the next launch, a persistent notification offers **Review report**. The preview shows the exact text that **Send to Goop** will put into a NIP-17 encrypted DM to the project's public key. The signer must be connected. Nothing is sent automatically. **Dismiss** deletes the selected reports; closing the preview leaves them available for the next launch.

A report contains the app version, build revision, OS/architecture, session start time, and either a Rust source filename/line or a Windows exception code/address. Panic messages, full source paths, application logs, credentials, conversation contents, and memory dumps are deliberately excluded. The first error in a process is retained. Up to five previous reports are offered at once, with each read bounded to 8 KiB.

Reports are removed only after explicit dismissal or successful insertion into the durable outgoing message queue. Queueing is not a claim of relay or recipient delivery: normal message delivery status and retry behavior apply. A failure to queue retains the report for retry.

This captures Rust panics and Windows exceptions that reach the process's unhandled-exception filter. It cannot guarantee a report for power loss, force termination, a loader failure before `main`, or all forms of severe process corruption. Native Unix signals are not captured by this implementation. Windows retains its normal OS crash handling.

Tests create a subprocess and an isolated temporary directory. They verify panic capture without payload leakage, next-start discovery, and report-specific dismissal. Windows CI additionally raises a native exception in that subprocess and checks that the OS filter saved its code. Tests never send a real DM or access the user's crash directory.
