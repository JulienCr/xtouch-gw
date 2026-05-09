//! Voicemeeter presence detector.
//!
//! Polls Win32 ToolHelp32Snapshot every `poll_interval` and broadcasts
//! transitions between [`VmState::Running`] and [`VmState::Absent`].
//!
//! The detector publishes its state through a `tokio::sync::watch` channel so
//! late subscribers always observe the latest known state without blocking.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Running,
    Absent,
}

pub trait ProcessScanner: Send + Sync + 'static {
    fn any_running(&self, names_lowercase: &[String]) -> bool;
}

pub struct VmDetector {
    state_rx: watch::Receiver<Option<VmState>>,
    _task: JoinHandle<()>,
}

impl VmDetector {
    pub fn spawn(
        scanner: Arc<dyn ProcessScanner>,
        process_names: Vec<String>,
        poll_interval: Duration,
    ) -> Self {
        let (state_tx, state_rx) = watch::channel(None);
        let names_lc: Vec<String> = process_names.iter().map(|s| s.to_lowercase()).collect();

        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;

                let names = names_lc.clone();
                let scanner = scanner.clone();
                let running = tokio::task::spawn_blocking(move || scanner.any_running(&names))
                    .await
                    .unwrap_or(false);
                let new_state = if running {
                    VmState::Running
                } else {
                    VmState::Absent
                };

                let prev = *state_tx.borrow();
                if prev != Some(new_state) {
                    info!(
                        "Voicemeeter state transition: {:?} -> {:?}",
                        prev, new_state
                    );
                    if state_tx.send(Some(new_state)).is_err() {
                        // No more receivers — exit detector loop.
                        return;
                    }
                }
            }
        });

        Self {
            state_rx,
            _task: task,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<VmState>> {
        self.state_rx.clone()
    }

    pub fn current(&self) -> Option<VmState> {
        *self.state_rx.borrow()
    }
}

#[cfg(target_os = "windows")]
pub struct Win32ProcessScanner;

#[cfg(target_os = "windows")]
impl ProcessScanner for Win32ProcessScanner {
    fn any_running(&self, names_lowercase: &[String]) -> bool {
        windows_impl::any_process_matches(names_lowercase)
    }
}

#[cfg(not(target_os = "windows"))]
pub struct NoopProcessScanner;

#[cfg(not(target_os = "windows"))]
impl ProcessScanner for NoopProcessScanner {
    fn any_running(&self, _: &[String]) -> bool {
        false
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use tracing::warn;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    pub fn any_process_matches(names_lc: &[String]) -> bool {
        unsafe {
            let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                Ok(h) => h,
                Err(e) => {
                    warn!("CreateToolhelp32Snapshot failed: {e}");
                    return false;
                },
            };

            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };

            let mut found = false;
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let exe_name = wchar_to_string(&entry.szExeFile).to_lowercase();
                    if names_lc.iter().any(|n| n == &exe_name) {
                        found = true;
                        break;
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }

            let _ = CloseHandle(snapshot);
            found
        }
    }

    fn wchar_to_string(wchars: &[u16]) -> String {
        let len = wchars.iter().position(|&c| c == 0).unwrap_or(wchars.len());
        String::from_utf16_lossy(&wchars[..len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct MockScanner {
        running: AtomicBool,
    }

    impl ProcessScanner for MockScanner {
        fn any_running(&self, _: &[String]) -> bool {
            self.running.load(Ordering::Acquire)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn initial_state_is_published() {
        let scanner = Arc::new(MockScanner {
            running: AtomicBool::new(true),
        });
        let detector = VmDetector::spawn(
            scanner,
            vec!["voicemeeter.exe".into()],
            Duration::from_millis(10),
        );

        let mut rx = detector.subscribe();
        // Wait for initial publication.
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), Some(VmState::Running));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn detects_running_to_absent() {
        let scanner = Arc::new(MockScanner {
            running: AtomicBool::new(true),
        });
        let detector = VmDetector::spawn(
            scanner.clone(),
            vec!["voicemeeter.exe".into()],
            Duration::from_millis(10),
        );

        let mut rx = detector.subscribe();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), Some(VmState::Running));

        scanner.running.store(false, Ordering::Release);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), Some(VmState::Absent));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn detects_absent_to_running() {
        let scanner = Arc::new(MockScanner {
            running: AtomicBool::new(false),
        });
        let detector = VmDetector::spawn(
            scanner.clone(),
            vec!["voicemeeter.exe".into()],
            Duration::from_millis(10),
        );

        let mut rx = detector.subscribe();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), Some(VmState::Absent));

        scanner.running.store(true, Ordering::Release);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), Some(VmState::Running));
    }
}
