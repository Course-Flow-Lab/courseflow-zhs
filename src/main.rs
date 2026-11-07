//! Process wiring: parse `CF_MODULE_SETTINGS`, build the 智慧树 implementation and
//! the `cw.*` host, then serve newline-delimited JSON-RPC on stdin/stdout until EOF.
//!
//! stdout is protocol-only. Every human-readable message goes to stderr.

use std::io;
use std::process;
use std::sync::Arc;

use serde_json::Value;

use courseflow_zhs::host::CwHost;
use courseflow_zhs::platform;
use courseflow_zhs::rpc::{self, Handler, RpcClient};
use courseflow_zhs::zhs::{Settings, Zhs};

fn main() {
    let raw_settings = std::env::var("CF_MODULE_SETTINGS").ok();
    let (settings, warnings) = Settings::from_env(raw_settings.as_deref());
    for warning in warnings {
        eprintln!("courseflow-zhs: {warning}");
    }

    let api = Arc::new(Zhs::new(settings));
    let client = Arc::new(RpcClient::new(io::stdout()));
    let host = Arc::new(CwHost::new(Arc::clone(&client)));

    let api_for_handler = Arc::clone(&api);
    let host_for_handler = Arc::clone(&host);
    let handler: Arc<Handler> = Arc::new(move |method: &str, params: &Value| {
        platform::dispatch(
            api_for_handler.as_ref(),
            host_for_handler.as_ref(),
            method,
            params,
        )
    });

    let reader = rpc::serve(client.clone(), io::BufReader::new(io::stdin()), handler);
    match reader {
        Ok(thread) => {
            let _ = thread.join();
        }
        Err(error) => {
            eprintln!("courseflow-zhs: 无法启动 stdin 读取线程：{error}");
            process::exit(1);
        }
    }
    client.flush();
    process::exit(0);
}
