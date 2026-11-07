//! Newline-delimited JSON-RPC 2.0 transport.
//!
//! One reader thread owns stdin. Incoming lines are one of:
//!   * a request from the framework -> dispatched on its own thread, then answered;
//!   * a response to one of our `cw.*` host calls -> completes the pending channel;
//!   * a notification -> logged to stderr (notifications never get a response).
//!
//! Handlers therefore never block the reader: a handler may itself block waiting for
//! a host response and the reader will still be there to receive it.
//!
//! stdout carries protocol lines only. Each line is serialized in full, written under
//! one mutex and flushed immediately. Human logging goes to stderr.

use std::{
    cell::Cell,
    collections::HashMap,
    io::{self, BufRead, BufWriter, Write},
    panic,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::error::ModuleError;

/// Hard budget for one framework -> module call. The framework kills at 30 s; we
/// stop issuing new host calls at 28 s so a typed error still gets written in time.
pub const CALL_BUDGET: Duration = Duration::from_secs(28);

/// Default wait for a single host call when no per-call deadline is installed.
pub const DEFAULT_HOST_CALL_TIMEOUT: Duration = Duration::from_secs(25);

/// A single protocol line larger than this is rejected rather than buffered forever.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// On stdin EOF, give in-flight handlers this long to write their response before
/// the process exits 0 (the framework's SIGTERM grace period is 5 s).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The module's request handler. Runs on its own thread for every incoming request.
pub type Handler = dyn Fn(&str, &Value) -> Result<Value, ModuleError> + Send + Sync + 'static;

thread_local! {
    static CALL_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn set_call_deadline(deadline: Option<Instant>) {
    CALL_DEADLINE.with(|cell| cell.set(deadline));
}

fn remaining_budget() -> Option<Duration> {
    CALL_DEADLINE.with(|cell| {
        cell.get()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    })
}

pub struct RpcClient {
    output: Mutex<BufWriter<Box<dyn Write + Send>>>,
    pending: Mutex<HashMap<u64, SyncSender<Result<Value, ModuleError>>>>,
    next_id: AtomicU64,
    active: Mutex<usize>,
    finished: Condvar,
}

impl RpcClient {
    pub fn new<W: Write + Send + 'static>(output: W) -> Self {
        Self {
            output: Mutex::new(BufWriter::new(Box::new(output))),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            active: Mutex::new(0),
            finished: Condvar::new(),
        }
    }

    fn begin_handler(&self) {
        *self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
    }

    fn end_handler(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        if *active == 0 {
            self.finished.notify_all();
        }
    }

    /// Wait for handlers that are already running, bounded by `timeout`.
    fn drain_handlers(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *active > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (guard, _) = self
                .finished
                .wait_timeout(active, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            active = guard;
        }
    }

    /// Perform one framework service call and wait for its response.
    ///
    /// The wait is capped by the smaller of the per-call default and the remainder of
    /// the current 30 s framework-call budget. It returns the raw `result` value.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, ModuleError> {
        let timeout = match remaining_budget() {
            Some(remaining) if remaining.is_zero() => {
                return Err(ModuleError::unavailable(format!(
                    "本次调用已用尽 30 秒预算，未能发起宿主调用 {method}"
                )));
            }
            Some(remaining) => remaining.min(DEFAULT_HOST_CALL_TIMEOUT),
            None => DEFAULT_HOST_CALL_TIMEOUT,
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, sender);

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_line(&message) {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            return Err(ModuleError::unavailable(format!(
                "无法写入协议行以调用 {method}：{error}"
            )));
        }

        match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                Err(ModuleError::unavailable(format!(
                    "宿主在 {} ms 内未响应 {method}",
                    timeout.as_millis()
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                Err(ModuleError::unavailable(format!(
                    "等待 {method} 的宿主响应时通道已断开"
                )))
            }
        }
    }

    /// Send a `cw.*` notification. Failures are only logged to stderr.
    pub fn notify(&self, method: &str, params: Value) {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_line(&message) {
            eprintln!("courseflow-zhs: 无法发送通知 {method}：{error}");
        }
    }

    /// Write a response for a framework request, echoing its id unchanged.
    pub fn send_response(&self, id: &Value, result: &Result<Value, ModuleError>) {
        let message = match result {
            Ok(value) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": value,
            }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error.to_json(),
            }),
        };
        if let Err(error) = self.write_line(&message) {
            eprintln!("courseflow-zhs: 无法写回响应（id={id}）：{error}");
        }
    }

    pub fn flush(&self) {
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(error) = output.flush() {
            eprintln!("courseflow-zhs: 刷新 stdout 失败：{error}");
        }
    }

    fn complete_host_call(&self, id: u64, result: Result<Value, ModuleError>) {
        let sender = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
        match sender {
            Some(sender) => {
                let _ = sender.send(result);
            }
            // A late reply after a timeout, or a reply we never asked for.
            None => eprintln!("courseflow-zhs: 忽略无主的宿主响应 id={id}"),
        }
    }

    fn write_line(&self, message: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push(b'\n');
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        output.write_all(&line)?;
        output.flush()
    }
}

/// Decrements the in-flight counter even if a handler panics past the catch.
struct HandlerGuard {
    client: Arc<RpcClient>,
}

impl Drop for HandlerGuard {
    fn drop(&mut self) {
        self.client.end_handler();
    }
}

/// Spawn the reader thread. It returns when stdin reaches EOF (or fails), at which
/// point the process is expected to exit 0 after flushing stdout.
pub fn serve<R>(
    client: Arc<RpcClient>,
    input: R,
    handler: Arc<Handler>,
) -> io::Result<JoinHandle<()>>
where
    R: BufRead + Send + 'static,
{
    thread::Builder::new()
        .name("cf-reader".to_owned())
        .spawn(move || reader_loop(client, input, handler))
}

fn reader_loop<R: BufRead>(client: Arc<RpcClient>, mut input: R, handler: Arc<Handler>) {
    let mut buffer = String::new();
    loop {
        buffer.clear();
        match input.read_line(&mut buffer) {
            Ok(0) => {
                // EOF from the framework: let in-flight handlers answer (the smoke
                // pipe closes stdin immediately after the request), then exit 0.
                client.drain_handlers(DRAIN_TIMEOUT);
                client.flush();
                return;
            }
            Ok(_) => {
                if buffer.len() > MAX_LINE_BYTES {
                    eprintln!(
                        "courseflow-zhs: 忽略超过 {} 字节的 stdin 行",
                        MAX_LINE_BYTES
                    );
                    continue;
                }
                let line = buffer.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(message) => handle_message(&client, &handler, message),
                    Err(error) => {
                        eprintln!("courseflow-zhs: 忽略无法解析的 stdin 行：{error}");
                    }
                }
            }
            Err(error) => {
                eprintln!("courseflow-zhs: stdin 读取失败：{error}");
                client.drain_handlers(DRAIN_TIMEOUT);
                client.flush();
                return;
            }
        }
    }
}

fn handle_message(client: &Arc<RpcClient>, handler: &Arc<Handler>, message: Value) {
    let Some(object) = message.as_object() else {
        eprintln!("courseflow-zhs: 忽略非对象 JSON-RPC 消息");
        return;
    };
    let method = object.get("method").and_then(Value::as_str);
    let id = object.get("id").cloned();

    match (method, id) {
        (Some(method), Some(id)) => {
            let method = method.to_owned();
            let params = object.get("params").cloned().unwrap_or(Value::Null);
            let call_client = Arc::clone(client);
            let call_handler = Arc::clone(handler);
            let call_id = id.clone();
            client.begin_handler();
            let spawned = thread::Builder::new()
                .name("cf-call".to_owned())
                .spawn(move || {
                    let _guard = HandlerGuard {
                        client: Arc::clone(&call_client),
                    };
                    // This thread may block on host calls; install the shared budget.
                    set_call_deadline(Some(Instant::now() + CALL_BUDGET));
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        call_handler(&method, &params)
                    }))
                    .unwrap_or_else(|_| {
                        Err(ModuleError::unavailable(
                            "模块处理请求时发生内部错误（已捕获 panic）",
                        ))
                    });
                    set_call_deadline(None);
                    call_client.send_response(&call_id, &result);
                });
            if let Err(error) = spawned {
                client.end_handler();
                eprintln!("courseflow-zhs: 无法为请求启动处理线程：{error}");
                client.send_response(
                    &id,
                    &Err(ModuleError::unavailable(format!(
                        "无法启动请求处理线程：{error}"
                    ))),
                );
            }
        }
        (Some(method), None) => {
            // Notifications are one-way: cw.log from the framework is for humans.
            if method == "cw.log" {
                let params = object.get("params").cloned().unwrap_or(Value::Null);
                let level = params
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("info");
                let text = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                eprintln!("courseflow-zhs: [{level}] {text}");
            } else if method != "cw.progress" {
                eprintln!("courseflow-zhs: 忽略未知通知 {method}");
            }
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                eprintln!("courseflow-zhs: 忽略非整数 id 的宿主响应");
                return;
            };
            let result = match object.get("error") {
                Some(error) => Err(host_error(error)),
                None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
            };
            client.complete_host_call(id, result);
        }
        (None, None) => {
            eprintln!("courseflow-zhs: 忽略既无 method 也无 id 的 JSON-RPC 消息");
        }
    }
}

fn host_error(error: &Value) -> ModuleError {
    let code = error.get("code").and_then(Value::as_str);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("宿主调用失败");
    ModuleError::from_wire(code, message)
}
