//! Minimal, opt-in diagnostics. Never record panic payloads, logs, keys, or memory dumps.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const LIMIT: u64 = 8192;
static RECORDER: OnceLock<Recorder> = OnceLock::new();
static PENDING: OnceLock<Vec<CrashReport>> = OnceLock::new();

struct Recorder {
    file: File,
    path: PathBuf,
    header: String,
    captured: AtomicBool,
}
#[derive(Clone, Debug)]
pub struct CrashReport {
    path: PathBuf,
    pub text: String,
}
impl CrashReport {
    /// Only remove the selected report, after dismissal or successful queueing.
    pub fn dismiss(&self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

pub fn pending() -> Vec<CrashReport> {
    PENDING.get().cloned().unwrap_or_default()
}

fn read_pending(directory: &Path) -> Vec<CrashReport> {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "crash"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .rev()
        .filter_map(|path| {
            let mut text = String::new();
            File::open(&path)
                .ok()?
                .take(LIMIT)
                .read_to_string(&mut text)
                .ok()?;
            if !text.starts_with("Goop crash report\n") || !text.contains("\nType: ") {
                return None;
            }
            Some(CrashReport { path, text })
        })
        .take(5)
        .collect()
}

pub struct CrashGuard;
impl Drop for CrashGuard {
    fn drop(&mut self) {
        if let Some(recorder) = RECORDER.get() {
            if !recorder.captured.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&recorder.path);
            }
        }
    }
}

/// Install before GPUI, fonts, logging, signer, or database initialization.
pub fn install(version: &str, revision: &str) -> CrashGuard {
    install_in(crate::support_dir().join("crashes"), version, revision)
}

fn install_in(directory: PathBuf, version: &str, revision: &str) -> CrashGuard {
    if fs::create_dir_all(&directory).is_err() {
        return CrashGuard;
    }
    let _ = PENDING.set(read_pending(&directory));
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = directory.join(format!("{time}-{}.crash", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let Ok(file) = options.open(&path) else {
        return CrashGuard;
    };
    let header = format!(
        "Goop crash report\nVersion: {version}\nBuild: {revision}\nPlatform: {} / {}\nSession started (Unix ms): {time}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if RECORDER
        .set(Recorder {
            file,
            path,
            header,
            captured: AtomicBool::new(false),
        })
        .is_err()
    {
        return CrashGuard;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(recorder) = RECORDER.get() {
            recorder.capture(
                "Rust panic",
                info.location().map(|l| (l.file(), l.line(), l.column())),
                None,
            );
        }
        previous(info);
    }));
    #[cfg(target_os = "windows")]
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(
            exception_filter,
        ));
    }
    CrashGuard
}

fn source_name(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or("unknown")
}
impl Recorder {
    fn capture(
        &self,
        kind: &str,
        location: Option<(&str, u32, u32)>,
        exception: Option<(u32, usize)>,
    ) {
        // First failure wins. No mutex, allocation, or arbitrary error formatting in the handler.
        if self.captured.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut file = &self.file;
        let _ = file.write_all(self.header.as_bytes());
        let _ = writeln!(file, "Type: {kind}");
        if let Some((name, line, column)) = location {
            let _ = writeln!(file, "Source: {}:{line}:{column}", source_name(name));
        }
        if let Some((code, address)) = exception {
            let _ = writeln!(file, "Exception: 0x{code:08x}\nAddress: 0x{address:x}");
        }
        let _ = file.sync_all();
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn exception_filter(
    info: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    // Windows owns these pointers and guarantees their validity for this callback.
    if let Some(recorder) = RECORDER.get() {
        if let Some(info) = unsafe { info.as_ref() } {
            if let Some(record) = unsafe { info.ExceptionRecord.as_ref() } {
                recorder.capture(
                    "Windows exception",
                    None,
                    Some((
                        record.ExceptionCode as u32,
                        record.ExceptionAddress as usize,
                    )),
                );
            }
        }
    }
    0 // EXCEPTION_CONTINUE_SEARCH: retain normal OS crash handling.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subprocess_panic_is_detected_without_payload_or_user_path() {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_report::tests::crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("GOOP_TEST_CRASH_DIR", dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let reports = read_pending(dir.path());
        assert_eq!(
            reports.len(),
            1,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(reports[0].text.contains("Type: Rust panic"));
        assert!(!reports[0].text.contains("private message and nsec"));
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn native_windows_exception_is_saved_by_the_os_filter() {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_report::tests::crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("GOOP_TEST_CRASH_DIR", dir.path())
            .env("GOOP_TEST_NATIVE_CRASH", "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let reports = read_pending(dir.path());
        assert_eq!(
            reports.len(),
            1,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(reports[0].text.contains("Type: Windows exception"));
        assert!(reports[0].text.contains("Exception: 0xe000474f"));
    }
    #[test]
    #[ignore = "only run as an isolated subprocess"]
    fn crash_child() {
        let directory = PathBuf::from(
            std::env::var_os("GOOP_TEST_CRASH_DIR").expect("isolated crash directory"),
        );
        let _guard = install_in(directory, "test", "test");
        #[cfg(target_os = "windows")]
        if std::env::var_os("GOOP_TEST_NATIVE_CRASH").is_some() {
            use windows_sys::Win32::System::Diagnostics::Debug::*;
            unsafe {
                SetErrorMode(SEM_NOGPFAULTERRORBOX | SEM_FAILCRITICALERRORS);
                RaiseException(0xe000474f, 1, 0, std::ptr::null());
            }
            std::process::exit(42);
        }
        panic!("private message and nsec must never enter the report");
    }
    #[test]
    fn minimal_report_survives_restart_and_dismissal_is_specific() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.crash");
        let recorder = Recorder {
            file: File::create(&path).unwrap(),
            path: path.clone(),
            header: "Goop crash report\nVersion: test\n".into(),
            captured: AtomicBool::new(false),
        };
        recorder.capture(
            "Rust panic",
            Some(("C:\\Users\\PrivateName\\src\\login.rs", 15, 8)),
            None,
        );
        recorder.capture("Windows exception", None, Some((1, 2)));
        drop(recorder);
        fs::write(dir.path().join("2.crash"), "").unwrap();
        let reports = read_pending(dir.path());
        assert_eq!(reports.len(), 1);
        assert!(reports[0].text.contains("Source: login.rs:15:8"));
        assert!(!reports[0].text.contains("PrivateName"));
        assert!(!reports[0].text.contains("Windows exception"));
        reports[0].dismiss().unwrap();
        assert!(read_pending(dir.path()).is_empty());
        assert!(dir.path().join("2.crash").exists());
    }
    #[test]
    fn empty_truncated_and_unrelated_files_are_not_reports() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("1.crash"), "Goop crash report\nVersion: 1").unwrap();
        fs::write(
            dir.path().join("notes.txt"),
            "Goop crash report\nType: fake",
        )
        .unwrap();
        assert!(read_pending(dir.path()).is_empty());
    }
}
