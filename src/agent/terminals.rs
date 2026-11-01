use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::protocol::{CreateTerminal, ExitStatus};

pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
const READ_CHUNK: usize = 8192;

pub type OnChange = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    truncated: bool,
}

pub struct Proc {
    pub label: String,
    limit: usize,
    pid: u32,
    output: Mutex<Output>,
    exit: Mutex<Option<ExitStatus>>,
    done: flume::Receiver<()>,
}

impl Proc {
    pub fn snapshot(&self) -> (String, bool) {
        let output = self.output.lock().unwrap_or_else(|p| p.into_inner());
        (String::from_utf8_lossy(&output.bytes).into_owned(), output.truncated)
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.exit.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn is_running(&self) -> bool {
        self.exit_status().is_none()
    }

    fn append(&self, chunk: &[u8]) {
        let mut output = self.output.lock().unwrap_or_else(|p| p.into_inner());
        output.bytes.extend_from_slice(chunk);
        if output.bytes.len() > self.limit {
            let mut cut = output.bytes.len() - self.limit;
            while cut < output.bytes.len() && (output.bytes[cut] & 0xC0) == 0x80 {
                cut += 1;
            }
            output.bytes.drain(..cut);
            output.truncated = true;
        }
    }

    fn kill(&self) {
        if self.is_running() {
            unsafe {
                libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL);
            }
        }
    }

    pub async fn wait(&self) -> ExitStatus {
        let _ = self.done.recv_async().await;
        self.exit_status().unwrap_or_default()
    }
}

#[derive(Default)]
pub struct Terminals {
    next: AtomicU64,
    active: Mutex<HashMap<String, Arc<Proc>>>,
    shown: Mutex<HashMap<String, Arc<Proc>>>,
}

pub fn clip_limit(requested: Option<u64>) -> usize {
    requested.map(|n| n as usize).filter(|n| *n > 0).unwrap_or(DEFAULT_OUTPUT_LIMIT).min(DEFAULT_OUTPUT_LIMIT * 8)
}

impl Terminals {
    pub fn create(&self, request: &CreateTerminal, cwd: &Path, on_change: OnChange) -> std::io::Result<String> {
        use std::os::unix::process::CommandExt as _;
        let mut command = Command::new(&request.command);
        command
            .args(&request.args)
            .current_dir(cwd)
            .env("PATH", super::registry::search_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        for var in &request.env {
            command.env(&var.name, &var.value);
        }
        let mut child = command.spawn()?;
        let id = format!("term-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let (done_tx, done) = flume::bounded::<()>(1);
        let label = std::iter::once(request.command.clone()).chain(request.args.iter().cloned()).collect::<Vec<_>>().join(" ");
        let proc = Arc::new(Proc {
            label,
            limit: clip_limit(request.output_byte_limit),
            pid: child.id(),
            output: Mutex::default(),
            exit: Mutex::new(None),
            done,
        });
        let readers: Vec<Box<dyn std::io::Read + Send>> = [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut handles = Vec::new();
        for mut reader in readers {
            let (proc, on_change, id) = (proc.clone(), on_change.clone(), id.clone());
            handles.push(std::thread::spawn(move || {
                let mut buffer = [0u8; READ_CHUNK];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            proc.append(&buffer[..n]);
                            on_change(&id);
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(err) => {
                            eprintln!("forge: agent terminal read failed: {err}");
                            break;
                        }
                    }
                }
            }));
        }
        let (waiter, waiter_id) = (proc.clone(), id.clone());
        std::thread::spawn(move || {
            let status = child.wait();
            for handle in handles {
                let _ = handle.join();
            }
            let exit = match status {
                Ok(status) => {
                    use std::os::unix::process::ExitStatusExt as _;
                    ExitStatus { code: status.code(), signal: status.signal().map(signal_name) }
                }
                Err(err) => {
                    eprintln!("forge: agent terminal wait failed: {err}");
                    ExitStatus::default()
                }
            };
            *waiter.exit.lock().unwrap_or_else(|p| p.into_inner()) = Some(exit);
            drop(done_tx);
            on_change(&waiter_id);
        });
        self.active.lock().unwrap_or_else(|p| p.into_inner()).insert(id.clone(), proc.clone());
        self.shown.lock().unwrap_or_else(|p| p.into_inner()).insert(id.clone(), proc);
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Proc>> {
        self.active.lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
    }

    pub fn shown(&self, id: &str) -> Option<Arc<Proc>> {
        self.shown.lock().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
    }

    pub fn kill(&self, id: &str) -> bool {
        match self.get(id) {
            Some(proc) => {
                proc.kill();
                true
            }
            None => false,
        }
    }

    pub fn release(&self, id: &str) -> bool {
        let proc = self.active.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
        match proc {
            Some(proc) => {
                proc.kill();
                true
            }
            None => false,
        }
    }

    pub fn kill_all(&self) {
        let procs: Vec<_> = self.active.lock().unwrap_or_else(|p| p.into_inner()).drain().map(|(_, p)| p).collect();
        for proc in procs {
            proc.kill();
        }
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        self.kill_all();
    }
}

fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGTERM => "SIGTERM".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        other => format!("signal {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(terminals: &Terminals, command: &str, args: &[&str], limit: Option<u64>) -> Arc<Proc> {
        let request = CreateTerminal {
            command: command.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: Vec::new(),
            cwd: None,
            output_byte_limit: limit,
        };
        let id = terminals.create(&request, &std::env::temp_dir(), Arc::new(|_| {})).unwrap();
        terminals.get(&id).unwrap()
    }

    #[test]
    fn captures_output_and_exit_code() {
        let terminals = Terminals::default();
        let proc = run(&terminals, "sh", &["-c", "echo hello; echo oops >&2; exit 3"], None);
        let status = futures::executor::block_on(proc.wait());
        assert_eq!(status.code, Some(3));
        let (text, truncated) = proc.snapshot();
        assert!(text.contains("hello") && text.contains("oops"));
        assert!(!truncated);
    }

    #[test]
    fn output_is_limited_from_the_front() {
        let terminals = Terminals::default();
        let proc = run(&terminals, "sh", &["-c", "printf 'abcdefghij'"], Some(4));
        futures::executor::block_on(proc.wait());
        assert_eq!(proc.snapshot(), ("ghij".into(), true));
    }

    #[test]
    fn kill_and_release_stop_the_process() {
        let terminals = Terminals::default();
        let request = CreateTerminal { command: "sleep".into(), args: vec!["30".into()], env: Vec::new(), cwd: None, output_byte_limit: None };
        let id = terminals.create(&request, &std::env::temp_dir(), Arc::new(|_| {})).unwrap();
        assert!(terminals.kill(&id));
        let proc = terminals.get(&id).unwrap();
        let status = futures::executor::block_on(proc.wait());
        assert_eq!(status.signal.as_deref(), Some("SIGKILL"));
        assert!(terminals.release(&id));
        assert!(terminals.get(&id).is_none());
        assert!(terminals.shown(&id).is_some());
    }

    #[test]
    fn clip_limit_has_sane_bounds() {
        assert_eq!(clip_limit(None), DEFAULT_OUTPUT_LIMIT);
        assert_eq!(clip_limit(Some(0)), DEFAULT_OUTPUT_LIMIT);
        assert_eq!(clip_limit(Some(10)), 10);
        assert_eq!(clip_limit(Some(u64::MAX)), DEFAULT_OUTPUT_LIMIT * 8);
    }
}
